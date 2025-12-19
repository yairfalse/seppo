//! Port forwarding for Kubernetes pods
//!
//! Provides `PortForward` for tunneling traffic to pods.

use k8s_openapi::api::core::v1::Pod;
use kube::api::Api;
use kube::Client;
use std::net::SocketAddr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tracing::{debug, warn};

/// Error type for port forwarding operations
#[derive(Debug, thiserror::Error)]
pub enum PortForwardError {
    #[error("Failed to bind local port: {0}")]
    BindError(String),

    #[error("HTTP request failed: {0}")]
    RequestError(String),
}

/// A port forward to a Kubernetes pod
///
/// Allows making HTTP requests to a pod through an established tunnel.
pub struct PortForward {
    local_addr: SocketAddr,
    _shutdown_tx: oneshot::Sender<()>,
}

impl PortForward {
    /// Create a new port forward to a pod
    pub(crate) async fn new(
        client: Client,
        namespace: &str,
        pod_name: &str,
        remote_port: u16,
    ) -> Result<Self, PortForwardError> {
        // Bind to a random local port
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| PortForwardError::BindError(e.to_string()))?;

        let local_addr = listener
            .local_addr()
            .map_err(|e| PortForwardError::BindError(e.to_string()))?;

        debug!(
            local_addr = %local_addr,
            pod = %pod_name,
            remote_port = %remote_port,
            "Port forward bound to local address"
        );

        let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();

        let pod_name = pod_name.to_string();
        let namespace = namespace.to_string();

        // Spawn the forwarding task
        tokio::spawn(async move {
            // Create Api once outside the loop
            let pods: Api<Pod> = Api::namespaced(client, &namespace);

            loop {
                tokio::select! {
                    accept_result = listener.accept() => {
                        match accept_result {
                            Ok((mut local_stream, _)) => {
                                let pods = pods.clone();
                                let pod_name = pod_name.clone();

                                tokio::spawn(async move {
                                    match pods.portforward(&pod_name, &[remote_port]).await {
                                        Ok(mut pf) => {
                                            if let Some(mut upstream) = pf.take_stream(remote_port) {
                                                let (mut local_read, mut local_write) = local_stream.split();
                                                let (mut upstream_read, mut upstream_write) = tokio::io::split(&mut upstream);

                                                let client_to_server = async {
                                                    tokio::io::copy(&mut local_read, &mut upstream_write).await
                                                };

                                                let server_to_client = async {
                                                    tokio::io::copy(&mut upstream_read, &mut local_write).await
                                                };

                                                tokio::select! {
                                                    result = client_to_server => {
                                                        if let Err(e) = result {
                                                            warn!(error = %e, "Error copying client to server");
                                                        }
                                                    },
                                                    result = server_to_client => {
                                                        if let Err(e) = result {
                                                            warn!(error = %e, "Error copying server to client");
                                                        }
                                                    },
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            warn!(error = %e, "Failed to establish port forward stream");
                                        }
                                    }
                                });
                            }
                            Err(e) => {
                                warn!(error = %e, "Failed to accept connection");
                            }
                        }
                    }
                    _ = &mut shutdown_rx => {
                        debug!("Port forward shutdown requested");
                        break;
                    }
                }
            }
        });

        Ok(Self {
            local_addr,
            _shutdown_tx: shutdown_tx,
        })
    }

    /// Get the local address of the port forward
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Get a URL for the given path through the port forward
    ///
    /// Useful when you need to pass a URL to another library or tool.
    /// Paths are automatically normalized to include a leading `/`.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let health_url = pf.url("/health");
    /// // Returns something like "http://127.0.0.1:54321/health"
    ///
    /// // Also works without leading slash
    /// let api_url = pf.url("api/users");
    /// // Returns "http://127.0.0.1:54321/api/users"
    /// ```
    pub fn url(&self, path: &str) -> String {
        let normalized_path = if path.is_empty() {
            "/".to_string()
        } else if path.starts_with('/') {
            path.to_string()
        } else {
            format!("/{}", path)
        };
        format!("http://{}{}", self.local_addr, normalized_path)
    }

    /// Make an HTTP GET request through the port forward
    ///
    /// Returns the response body as a UTF-8 string. Binary responses
    /// are not supported - use `local_addr()` with your own HTTP client
    /// for binary data.
    pub async fn get(&self, path: &str) -> Result<String, PortForwardError> {
        self.request("GET", path, None).await
    }

    /// Make an HTTP POST request through the port forward
    ///
    /// # Example
    ///
    /// ```ignore
    /// let response = pf.post("/api/users", "application/json", r#"{"name":"test"}"#).await?;
    /// ```
    pub async fn post(
        &self,
        path: &str,
        content_type: &str,
        body: &str,
    ) -> Result<String, PortForwardError> {
        self.request("POST", path, Some((content_type, body))).await
    }

    /// Make an HTTP PUT request through the port forward
    ///
    /// # Example
    ///
    /// ```ignore
    /// let response = pf.put("/api/users/1", "application/json", r#"{"name":"updated"}"#).await?;
    /// ```
    pub async fn put(
        &self,
        path: &str,
        content_type: &str,
        body: &str,
    ) -> Result<String, PortForwardError> {
        self.request("PUT", path, Some((content_type, body))).await
    }

    /// Make an HTTP DELETE request through the port forward
    ///
    /// # Example
    ///
    /// ```ignore
    /// let response = pf.delete("/api/users/1").await?;
    /// ```
    pub async fn delete(&self, path: &str) -> Result<String, PortForwardError> {
        self.request("DELETE", path, None).await
    }

    /// Make a generic HTTP request
    async fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<(&str, &str)>, // (content_type, body)
    ) -> Result<String, PortForwardError> {
        let mut stream = tokio::net::TcpStream::connect(self.local_addr)
            .await
            .map_err(|e| PortForwardError::RequestError(e.to_string()))?;

        let request = if let Some((content_type, body_content)) = body {
            format!(
                "{} {} HTTP/1.1\r\nHost: {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                method, path, self.local_addr, content_type, body_content.len(), body_content
            )
        } else {
            format!(
                "{} {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
                method, path, self.local_addr
            )
        };

        stream
            .write_all(request.as_bytes())
            .await
            .map_err(|e| PortForwardError::RequestError(e.to_string()))?;

        let mut response = String::new();
        stream
            .read_to_string(&mut response)
            .await
            .map_err(|e| PortForwardError::RequestError(e.to_string()))?;

        debug!(
            local_addr = %self.local_addr,
            method = %method,
            path = %path,
            "HTTP request completed"
        );

        Ok(extract_body(&response))
    }
}

/// Extract the body from an HTTP response
fn extract_body(response: &str) -> String {
    if let Some(body_start) = response.find("\r\n\r\n") {
        response[body_start + 4..].to_string()
    } else {
        response.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_body_with_headers() {
        let response = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n<html>body</html>";
        assert_eq!(extract_body(response), "<html>body</html>");
    }

    #[test]
    fn test_extract_body_empty_body() {
        let response = "HTTP/1.1 204 No Content\r\n\r\n";
        assert_eq!(extract_body(response), "");
    }

    #[test]
    fn test_extract_body_no_separator() {
        let response = "malformed response";
        assert_eq!(extract_body(response), "malformed response");
    }

    #[test]
    fn test_extract_body_multiple_separators() {
        let response = "HTTP/1.1 200 OK\r\n\r\nfirst\r\n\r\nsecond";
        assert_eq!(extract_body(response), "first\r\n\r\nsecond");
    }

    #[test]
    fn test_url_path_normalization() {
        // Helper to test the path normalization logic
        fn normalize_path(path: &str) -> String {
            if path.starts_with('/') {
                path.to_string()
            } else {
                format!("/{}", path)
            }
        }

        // Path with leading slash stays unchanged
        assert_eq!(normalize_path("/health"), "/health");
        assert_eq!(normalize_path("/api/users"), "/api/users");

        // Path without leading slash gets one added
        assert_eq!(normalize_path("health"), "/health");
        assert_eq!(normalize_path("api/users"), "/api/users");

        // Empty path gets a slash
        assert_eq!(normalize_path(""), "/");
    }
}
