//! Test scenarios for organized integration tests
//!
//! Provides a way to structure related tests with shared setup.
//!
//! # Example
//!
//! ```ignore
//! use seppo::scenario::Scenario;
//!
//! #[tokio::test]
//! async fn test_user_api() {
//!     Scenario::new("user-api")
//!         .given("a running user service", || async {
//!             // Setup deployment here
//!             // e.g., DeploymentFixture::new("user-svc").image("user-svc:test").port(8080).build();
//!             Ok(())
//!         })
//!         .when("creating a user", || async {
//!             // Perform user creation here
//!             // e.g., let resp = ...;
//!             Ok(())
//!         })
//!         .then("the user should be created", |resp| {
//!             assert!(resp.contains("id"));
//!         })
//!         .run()
//!         .await?;
//! }
//! ```

use std::fmt;
use std::future::Future;
use std::pin::Pin;

/// Error type for scenario operations
#[derive(Debug, thiserror::Error)]
pub enum ScenarioError {
    #[error("setup failed: {0}")]
    SetupFailed(String),

    #[error("action failed: {0}")]
    ActionFailed(String),

    #[error("assertion failed: {0}")]
    AssertionFailed(String),

    #[error("teardown failed: {0}")]
    TeardownFailed(String),
}

/// Type alias for async action returning `Result<T, ScenarioError>`
type AsyncAction<T> =
    Box<dyn FnOnce() -> Pin<Box<dyn Future<Output = Result<T, ScenarioError>> + Send>> + Send>;

/// A test scenario that structures BDD-style tests
pub struct Scenario<T = ()> {
    name: String,
    given_desc: Option<String>,
    when_desc: Option<String>,
    then_desc: Option<String>,
    given: Option<AsyncAction<()>>,
    when: Option<AsyncAction<T>>,
    then: Option<Box<dyn FnOnce(T) + Send>>,
}

impl Scenario<()> {
    /// Create a new scenario with a name
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            given_desc: None,
            when_desc: None,
            then_desc: None,
            given: None,
            when: None,
            then: None,
        }
    }
}

impl<T: Send + 'static> Scenario<T> {
    /// Define the precondition (Given)
    #[must_use]
    pub fn given<F, Fut>(mut self, description: &str, setup: F) -> Self
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), ScenarioError>> + Send + 'static,
    {
        self.given_desc = Some(description.to_string());
        self.given = Some(Box::new(move || Box::pin(setup())));
        self
    }

    /// Define the action (When) and return a new scenario with the result type
    #[must_use]
    pub fn when<F, Fut, R>(self, description: &str, action: F) -> Scenario<R>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<R, ScenarioError>> + Send + 'static,
        R: Send + 'static,
    {
        Scenario {
            name: self.name,
            given_desc: self.given_desc,
            when_desc: Some(description.to_string()),
            then_desc: None,
            given: self.given,
            when: Some(Box::new(move || Box::pin(action()))),
            then: None,
        }
    }

    /// Define the assertion (Then)
    #[must_use]
    pub fn then<F>(mut self, description: &str, assertion: F) -> Self
    where
        F: FnOnce(T) + Send + 'static,
    {
        self.then_desc = Some(description.to_string());
        self.then = Some(Box::new(assertion));
        self
    }

    /// Run the scenario
    ///
    /// # Errors
    ///
    /// Returns `ScenarioError` if any step (given/when/then) fails.
    pub async fn run(self) -> Result<(), ScenarioError> {
        println!("\n📋 Scenario: {}", self.name);

        // Run Given
        if let Some(setup) = self.given {
            if let Some(desc) = &self.given_desc {
                println!("   Given {desc}");
            }
            setup().await?;
        }

        // Run When
        let result = if let Some(action) = self.when {
            if let Some(desc) = &self.when_desc {
                println!("   When {desc}");
            }
            Some(action().await?)
        } else {
            None
        };

        // Run Then
        if let Some(assertion) = self.then {
            if let Some(desc) = &self.then_desc {
                println!("   Then {desc}");
            }
            match result {
                Some(result) => {
                    let assertion_result =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            assertion(result);
                        }));
                    if let Err(panic) = assertion_result {
                        let msg = if let Some(s) = panic.downcast_ref::<&str>() {
                            (*s).to_string()
                        } else if let Some(s) = panic.downcast_ref::<String>() {
                            s.clone()
                        } else {
                            "assertion panicked".to_string()
                        };
                        return Err(ScenarioError::AssertionFailed(msg));
                    }
                }
                None => {
                    return Err(ScenarioError::AssertionFailed(
                        "Then assertion defined but no result from When step to assert on"
                            .to_string(),
                    ));
                }
            }
        }

        println!("   ✅ Scenario passed\n");
        Ok(())
    }
}

/// A simpler step-based scenario builder
#[derive(Default)]
pub struct Steps {
    name: String,
    steps: Vec<Step>,
}

struct Step {
    description: String,
    action: AsyncAction<()>,
}

impl Steps {
    /// Create a new steps-based scenario
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            steps: Vec::new(),
        }
    }

    /// Add a step to the scenario
    #[must_use]
    pub fn step<F, Fut>(mut self, description: &str, action: F) -> Self
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), ScenarioError>> + Send + 'static,
    {
        self.steps.push(Step {
            description: description.to_string(),
            action: Box::new(move || Box::pin(action())),
        });
        self
    }

    /// Run all steps in order
    ///
    /// # Errors
    ///
    /// Returns `ScenarioError` if any step fails.
    pub async fn run(self) -> Result<(), ScenarioError> {
        println!("\n📋 Scenario: {}", self.name);

        for (i, step) in self.steps.into_iter().enumerate() {
            println!("   {}. {}", i + 1, step.description);
            (step.action)().await?;
        }

        println!("   ✅ All steps passed\n");
        Ok(())
    }
}

impl fmt::Debug for Steps {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Steps")
            .field("name", &self.name)
            .field("step_count", &self.steps.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_scenario_basic() {
        let result = Scenario::new("basic test")
            .given("nothing special", || async { Ok(()) })
            .when("we do nothing", || async { Ok("result") })
            .then("everything is fine", |result| {
                assert_eq!(result, "result");
            })
            .run()
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_scenario_given_fails() {
        let result = Scenario::new("failing setup")
            .given("a failing condition", || async {
                Err(ScenarioError::SetupFailed("intentional".to_string()))
            })
            .when("we try something", || async { Ok(()) })
            .run()
            .await;

        assert!(matches!(result, Err(ScenarioError::SetupFailed(_))));
    }

    #[tokio::test]
    async fn test_scenario_when_fails() {
        let result = Scenario::new("failing action")
            .given("everything is ready", || async { Ok(()) })
            .when("the action fails", || async {
                Err::<(), _>(ScenarioError::ActionFailed("intentional".to_string()))
            })
            .run()
            .await;

        assert!(matches!(result, Err(ScenarioError::ActionFailed(_))));
    }

    #[tokio::test]
    async fn test_steps_basic() {
        // We can't easily capture mutable state in the closures, so just test structure
        let result = Steps::new("step-based test")
            .step("first step", || async { Ok(()) })
            .step("second step", || async { Ok(()) })
            .step("third step", || async { Ok(()) })
            .run()
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_steps_fails_on_error() {
        let result = Steps::new("failing steps")
            .step("succeeds", || async { Ok(()) })
            .step("fails", || async {
                Err(ScenarioError::ActionFailed("step failed".to_string()))
            })
            .step("never runs", || async { Ok(()) })
            .run()
            .await;

        assert!(matches!(result, Err(ScenarioError::ActionFailed(_))));
    }

    #[test]
    fn test_scenario_error_display() {
        assert_eq!(
            ScenarioError::SetupFailed("test".to_string()).to_string(),
            "setup failed: test"
        );
        assert_eq!(
            ScenarioError::ActionFailed("test".to_string()).to_string(),
            "action failed: test"
        );
        assert_eq!(
            ScenarioError::AssertionFailed("test".to_string()).to_string(),
            "assertion failed: test"
        );
        assert_eq!(
            ScenarioError::TeardownFailed("test".to_string()).to_string(),
            "teardown failed: test"
        );
    }

    #[tokio::test]
    async fn test_scenario_assertion_panic_caught() {
        let result = Scenario::new("panicking assertion")
            .given("nothing special", || async { Ok(()) })
            .when("we do something", || async { Ok(42) })
            .then("assertion panics", |_result| {
                panic!("intentional panic in assertion");
            })
            .run()
            .await;

        assert!(matches!(result, Err(ScenarioError::AssertionFailed(_))));
        if let Err(ScenarioError::AssertionFailed(msg)) = result {
            assert!(msg.contains("intentional panic"));
        }
    }
}
