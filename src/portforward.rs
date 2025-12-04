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
    #[error("Failed to create port forward: {0}")]
    CreateError(String),

    #[error("Failed to bind local port: {0}")]
    BindError(String),

    #[error("HTTP request failed: {0}")]
    RequestError(String),

    #[error("Port forward not ready")]
    NotReady,
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
            loop {
                tokio::select! {
                    accept_result = listener.accept() => {
                        match accept_result {
                            Ok((mut local_stream, _)) => {
                                let pods = Api::<Pod>::namespaced(client.clone(), &namespace);
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
                                                    _ = client_to_server => {},
                                                    _ = server_to_client => {},
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

    /// Make an HTTP GET request through the port forward
    pub async fn get(&mut self, path: &str) -> Result<String, PortForwardError> {
        let url = format!("http://{}{}", self.local_addr, path);

        // Simple HTTP/1.1 GET request
        let mut stream = tokio::net::TcpStream::connect(self.local_addr)
            .await
            .map_err(|e| PortForwardError::RequestError(e.to_string()))?;

        let request = format!(
            "GET {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
            path, self.local_addr
        );

        stream
            .write_all(request.as_bytes())
            .await
            .map_err(|e| PortForwardError::RequestError(e.to_string()))?;

        let mut response = String::new();
        stream
            .read_to_string(&mut response)
            .await
            .map_err(|e| PortForwardError::RequestError(e.to_string()))?;

        debug!(url = %url, "HTTP GET completed");

        // Extract body from response (after \r\n\r\n)
        if let Some(body_start) = response.find("\r\n\r\n") {
            Ok(response[body_start + 4..].to_string())
        } else {
            Ok(response)
        }
    }
}
