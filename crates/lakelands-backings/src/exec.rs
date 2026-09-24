//! The `exec` backing: a local command the operator configured, with the
//! call on standard input and the output on standard output.
//!
//! A function names the command in its configuration
//! (`config: { command: <name> }`); the program and its arguments are the
//! operator's, from [`ExecConfig`], and a name the operator did not
//! configure is refused. The program is started directly, no shell, with
//! an empty environment, so nothing of the server's own environment
//! reaches it. It receives `{ "function", "inputs", "config" }` as JSON
//! on standard input and answers one JSON value on standard output. A
//! run that exits with a failure status, takes longer than the timeout or
//! writes more than the limit fails the call, and is killed in the last
//! two cases. A failure is recorded as its exit status and a fixed
//! sentence; what the command writes on standard error goes to the
//! server's own standard error, never into the log.

use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use lakelands_api::Result;
use lakelands_api::seam::{FunctionBacking, FunctionCall};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::{ExecConfig, body, failed, output, refused};

/// The backing kind.
pub const KIND: &str = "exec";

/// The `exec` backing.
#[derive(Clone, Debug)]
pub struct Exec {
    config: ExecConfig,
}

impl Exec {
    /// The backing, allowed the commands of `config`.
    pub fn new(config: ExecConfig) -> Exec {
        Exec { config }
    }

    async fn run(&self, call: &FunctionCall) -> Result<Value> {
        let name = call.config["command"].as_str().ok_or_else(|| {
            refused(
                call,
                "an `exec` function names its command as `config.command`".into(),
            )
        })?;
        let command = self.config.commands.get(name).ok_or_else(|| {
            refused(
                call,
                format!("the command `{name}` is not one the operator allows"),
            )
        })?;
        let mut child = tokio::process::Command::new(&command.program)
            .args(&command.args)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // What the command says on standard error is for the operator:
            // it goes to the server's own standard error, never into the
            // log, where a field's metadata would show it to every reader.
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| failed(call, format!("the command `{name}` did not start: {e}")))?;
        let mut stdin = child.stdin.take().expect("stdin is piped");
        let stdout = child.stdout.take().expect("stdout is piped");
        let limit = self.config.max_output_bytes;
        let input = body(call);
        let timeout = Duration::from_secs(self.config.timeout_secs);
        let exchange = async {
            // The command may answer before it reads all its input; a
            // closed pipe then is not the call's failure.
            let write = async {
                let _ = stdin.write_all(&input).await;
                drop(stdin);
            };
            let mut out = Vec::new();
            let mut pipe = stdout.take(limit.saturating_add(1));
            let read = pipe.read_to_end(&mut out);
            let ((), read) = tokio::join!(write, read);
            read.map_err(|_| failed(call, format!("the output of `{name}` could not be read")))?;
            if out.len() as u64 > limit {
                // Past the limit: stopped at once, not at the timeout, and
                // reaped here.
                let _ = child.kill().await;
                return Err(failed(
                    call,
                    format!("the command `{name}` wrote more than {limit} bytes"),
                ));
            }
            let status = child.wait().await.map_err(|_| {
                failed(
                    call,
                    format!("the command `{name}` could not be waited for"),
                )
            })?;
            Ok((status, out))
        };
        let answer = tokio::time::timeout(timeout, exchange).await;
        let (status, out) = match answer {
            Ok(answer) => answer?,
            Err(_) => {
                // Stopped and reaped here, not left to the runtime.
                let _ = child.kill().await;
                return Err(failed(
                    call,
                    format!(
                        "the command `{name}` took longer than {}s and was stopped",
                        timeout.as_secs()
                    ),
                ));
            }
        };
        if !status.success() {
            return Err(failed(
                call,
                match status.code() {
                    Some(code) => format!("the command `{name}` failed with exit status {code}"),
                    None => format!("the command `{name}` was stopped by a signal"),
                },
            ));
        }
        output(call, &out)
    }
}

#[async_trait]
impl FunctionBacking for Exec {
    fn kind(&self) -> &str {
        KIND
    }

    async fn call(&self, call: &FunctionCall) -> Result<Value> {
        self.run(call).await
    }
}
