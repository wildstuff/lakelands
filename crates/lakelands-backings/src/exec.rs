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
use lakelands_api::error::{Case, Detail, Exchange};
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
        let name = call.config["command"].as_str();
        let name = name.ok_or_else(|| refused(call, Case::ExecNoCommand))?;
        let command = self.config.commands.get(name).ok_or_else(|| {
            let command = name.to_owned();
            refused(call, Case::CommandNotAllowed { command })
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
            .map_err(|e| {
                let (command, detail) = (name.to_owned(), Detail::io(&e));
                failed(call, Exchange::DidNotStart { command, detail })
            })?;
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
            let command = name.to_owned();
            read.map_err(|_| failed(call, Exchange::OutputUnread { command }))?;
            if out.len() as u64 > limit {
                // Past the limit: stopped at once, not at the timeout, and
                // reaped here.
                let _ = child.kill().await;
                let (command, cap) = (name.to_owned(), limit);
                return Err(failed(call, Exchange::OutputTooLarge { command, cap }));
            }
            let status = child.wait().await.map_err(|_| {
                let command = name.to_owned();
                failed(call, Exchange::NotWaited { command })
            })?;
            Ok((status, out))
        };
        let answer = tokio::time::timeout(timeout, exchange).await;
        let (status, out) = match answer {
            Ok(answer) => answer?,
            Err(_) => {
                // Stopped and reaped here, not left to the runtime.
                let _ = child.kill().await;
                let (command, secs) = (name.to_owned(), timeout.as_secs());
                return Err(failed(call, Exchange::TookTooLong { command, secs }));
            }
        };
        if !status.success() {
            let (command, exit_status) = (name.to_owned(), status.code());
            return Err(failed(
                call,
                Exchange::Exited {
                    command,
                    exit_status,
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
