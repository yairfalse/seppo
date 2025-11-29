//! Test runner for executing test commands
//!
//! Handles the "RUN TESTS" step of Seppo's test lifecycle:
//! - Execute user's test command
//! - Capture stdout/stderr
//! - Report exit code and pass/fail status

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_run_result_passed() {
        let result = RunResult {
            exit_code: 0,
            stdout: "All tests passed".to_string(),
            stderr: String::new(),
        };
        assert!(result.passed());
    }

    #[test]
    fn test_run_result_failed() {
        let result = RunResult {
            exit_code: 1,
            stdout: String::new(),
            stderr: "Test failed".to_string(),
        };
        assert!(!result.passed());
    }

    #[tokio::test]
    async fn test_run_simple_command() {
        // Run a simple echo command
        let result = run("echo", &["hello"]).await.unwrap();

        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.contains("hello"));
        assert!(result.passed());
    }

    #[tokio::test]
    async fn test_run_failing_command() {
        // Run a command that fails
        let result = run("sh", &["-c", "exit 1"]).await.unwrap();

        assert_eq!(result.exit_code, 1);
        assert!(!result.passed());
    }

    #[tokio::test]
    async fn test_run_captures_stderr() {
        // Run a command that writes to stderr
        let result = run("sh", &["-c", "echo error >&2"]).await.unwrap();

        assert!(result.stderr.contains("error"));
    }

    #[tokio::test]
    async fn test_run_with_env() {
        // Run a command with environment variables
        let mut env = std::collections::HashMap::new();
        env.insert("MY_VAR".to_string(), "my_value".to_string());

        let result = run_with_env("sh", &["-c", "echo $MY_VAR"], &env).await.unwrap();

        assert!(result.stdout.contains("my_value"));
    }

    #[tokio::test]
    async fn test_run_command_not_found() {
        // Run a command that doesn't exist
        let result = run("nonexistent_command_xyz", &[]).await;

        assert!(result.is_err());
    }

    #[test]
    fn test_runner_error_display() {
        let err = RunnerError::CommandNotFound("missing".to_string());
        assert!(err.to_string().contains("missing"));

        let err = RunnerError::ExecutionFailed("spawn error".to_string());
        assert!(err.to_string().contains("spawn"));
    }
}
