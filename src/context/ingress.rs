use k8s_openapi::api::networking::v1::Ingress;
use kube::api::Api;
use kube::Client;

/// Fluent test helper for asserting Ingress resource configuration
///
/// Created via `ctx.test_ingress("name")`. Chain assertions with
/// `.host()`, `.path()`, `.expect_backend()`, `.expect_tls()`, and
/// finalize with `.must()`.
///
/// # Example
///
/// ```ignore
/// ctx.test_ingress("my-ingress")
///     .host("example.com")
///     .path("/api")
///     .expect_backend("svc/backend", 8080).await
///     .expect_tls("my-tls-secret").await
///     .must(); // Panics if any assertion failed
/// ```
pub struct IngressTest {
    client: Client,
    namespace: String,
    name: String,
    host: Option<String>,
    path: Option<String>,
    err: Option<String>,
}

impl IngressTest {
    pub(crate) fn new(client: Client, namespace: String, name: String) -> Self {
        Self {
            client,
            namespace,
            name,
            host: None,
            path: None,
            err: None,
        }
    }

    /// Set the host to test
    #[must_use]
    pub fn host(mut self, host: &str) -> Self {
        if self.err.is_some() {
            return self;
        }
        self.host = Some(host.to_string());
        self
    }

    /// Set the path to test
    #[must_use]
    pub fn path(mut self, path: &str) -> Self {
        if self.err.is_some() {
            return self;
        }
        self.path = Some(path.to_string());
        self
    }

    /// Assert the ingress routes to the expected backend
    pub async fn expect_backend(mut self, backend: &str, port: u16) -> Self {
        if self.err.is_some() {
            return self;
        }

        let api: Api<Ingress> = Api::namespaced(self.client.clone(), &self.namespace);

        match api.get(&self.name).await {
            Ok(ingress) => {
                let expected_svc = backend.strip_prefix("svc/").unwrap_or(backend);

                let Some(spec) = ingress.spec else {
                    self.err = Some(format!("Ingress {} has no spec", self.name));
                    return self;
                };

                let mut found = false;
                for rule in spec.rules.unwrap_or_default() {
                    // Check host match (None in test means match any)
                    if let Some(ref test_host) = self.host {
                        if rule.host.as_ref() != Some(test_host) {
                            continue;
                        }
                    }

                    if let Some(http) = rule.http {
                        // Path matching semantics:
                        // - If self.path is None, match any ingress path.
                        // - If the ingress path is empty, treat it as matching all request paths.
                        // - Otherwise, require that the ingress path is a path-segment prefix of
                        //   the requested path, e.g. "/api" matches "/api", "/api/", "/api/v1"
                        //   but does NOT match "/apiv2".
                        let path_prefix_matches = |test_path: &str, ingress_path: &str| {
                            if ingress_path.is_empty() {
                                // Empty ingress path behaves as a catch-all.
                                true
                            } else if test_path == ingress_path {
                                true
                            } else if let Some(rest) = test_path.strip_prefix(ingress_path) {
                                rest.starts_with('/')
                            } else {
                                false
                            }
                        };

                        for p in http.paths {
                            // Check path match (None in test means match any)
                            let ingress_path = p.path.as_deref().unwrap_or("");
                            let path_matches = self.path.as_deref().is_none_or(|test_path| {
                                path_prefix_matches(test_path, ingress_path)
                            });

                            if path_matches {
                                if let Some(service) = p.backend.service {
                                    let actual_svc = service.name;
                                    let actual_port =
                                        service.port.and_then(|p| p.number).unwrap_or(0);

                                    if actual_svc == expected_svc
                                        && actual_port == i32::from(port)
                                    {
                                        found = true;
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    if found {
                        break;
                    }
                }

                if !found {
                    self.err = Some(format!(
                        "Ingress {} does not route host={:?} path={:?} to {}:{}",
                        self.name, self.host, self.path, expected_svc, port
                    ));
                }
            }
            Err(e) => {
                self.err = Some(format!("Failed to get ingress {}: {}", self.name, e));
            }
        }

        self
    }

    /// Assert the ingress has TLS configured with the given secret
    pub async fn expect_tls(mut self, secret_name: &str) -> Self {
        if self.err.is_some() {
            return self;
        }

        let api: Api<Ingress> = Api::namespaced(self.client.clone(), &self.namespace);

        match api.get(&self.name).await {
            Ok(ingress) => {
                let Some(spec) = ingress.spec else {
                    self.err = Some(format!("Ingress {} has no spec", self.name));
                    return self;
                };

                let mut found = false;
                for tls in spec.tls.unwrap_or_default() {
                    if tls.secret_name.as_ref() == Some(&secret_name.to_string()) {
                        // Check if host matches (if specified)
                        if self.host.is_none() {
                            found = true;
                            break;
                        }
                        for host in tls.hosts.unwrap_or_default() {
                            if Some(&host) == self.host.as_ref() {
                                found = true;
                                break;
                            }
                        }
                    }
                    if found {
                        break;
                    }
                }

                if !found {
                    self.err = Some(format!(
                        "Ingress {} does not have TLS secret {} for host {:?}",
                        self.name, secret_name, self.host
                    ));
                }
            }
            Err(e) => {
                self.err = Some(format!("Failed to get ingress {}: {}", self.name, e));
            }
        }

        self
    }

    /// Get any error from the test chain
    #[must_use]
    pub fn error(&self) -> Option<&str> {
        self.err.as_deref()
    }

    /// Panic if there was an error in the test chain
    pub fn must(self) {
        if let Some(err) = self.err {
            panic!("IngressTest failed: {err}");
        }
    }
}
