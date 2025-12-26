//! Test runner for executing test commands
//!
//! Handles the "RUN TESTS" step of Seppo's test lifecycle:
//! - Execute user's test command
//! - Capture stdout/stderr
//! - Report exit code and pass/fail status

use std::collections::HashMap;
use std::process::Command;
use tracing::{debug, info, instrument};

/// Result of running a test command
#[derive(Debug)]
pub struct RunResult {
    /// Exit code from the command (0 = success)
    pub exit_code: i32,
    /// Standard output
    pub stdout: String,
    /// Standard error
    pub stderr: String,
}

impl RunResult {
    /// Returns true if the command succeeded (exit code 0)
    #[must_use]
    pub fn passed(&self) -> bool {
        self.exit_code == 0
    }
}

/// Runner errors
#[derive(Debug, thiserror::Error)]
pub enum RunnerError {
    #[error("Command not found: {0}")]
    CommandNotFound(String),

    #[error("Execution failed: {0}")]
    ExecutionFailed(String),
}

/// Run a command and capture its output
///
/// # Arguments
/// * `program` - The program to run
/// * `args` - Command line arguments
///
/// # Returns
/// * `RunResult` with exit code, stdout, and stderr
///
/// # Errors
///
/// Returns `RunnerError::CommandNotFound` if the program doesn't exist,
/// or `RunnerError::ExecutionFailed` if execution fails.
#[instrument(fields(program = %program))]
pub async fn run(program: &str, args: &[&str]) -> Result<RunResult, RunnerError> {
    run_with_env(program, args, &HashMap::new()).await
}

/// Run a command with environment variables
///
/// # Arguments
/// * `program` - The program to run
/// * `args` - Command line arguments
/// * `env` - Environment variables to set
///
/// # Returns
/// * `RunResult` with exit code, stdout, and stderr
///
/// # Errors
///
/// Returns `RunnerError::CommandNotFound` if the program doesn't exist,
/// or `RunnerError::ExecutionFailed` if execution fails.
#[instrument(skip(env), fields(program = %program))]
#[allow(clippy::implicit_hasher)] // Simplicity over generalization for this helper
pub async fn run_with_env(
    program: &str,
    args: &[&str],
    env: &HashMap<String, String>,
) -> Result<RunResult, RunnerError> {
    info!("Running: {} {}", program, args.join(" "));

    let mut cmd = Command::new(program);
    cmd.args(args);

    // Add environment variables
    for (key, value) in env {
        cmd.env(key, value);
    }

    let output = cmd.output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            RunnerError::CommandNotFound(program.to_string())
        } else {
            RunnerError::ExecutionFailed(e.to_string())
        }
    })?;

    let exit_code = output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if exit_code == 0 {
        debug!("Command succeeded");
    } else {
        debug!("Command failed with exit code {}", exit_code);
    }

    Ok(RunResult {
        exit_code,
        stdout,
        stderr,
    })
}

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

        let result = run_with_env("sh", &["-c", "echo $MY_VAR"], &env)
            .await
            .unwrap();

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
