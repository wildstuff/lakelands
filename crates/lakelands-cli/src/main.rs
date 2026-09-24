//! The `lakelands` binary: parses the command line and runs it on one
//! thread, opening lakes with [`lakelands_cli::open_store`].

use std::process::ExitCode;

fn main() -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_current_thread().build() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: could not start: {e}");
            return ExitCode::FAILURE;
        }
    };
    let (mut out, mut err) = (std::io::stdout().lock(), std::io::stderr().lock());
    let code = runtime.block_on(lakelands_cli::run(
        std::env::args_os(),
        &mut out,
        &mut err,
        &lakelands_cli::open_store,
    ));
    ExitCode::from(code)
}
