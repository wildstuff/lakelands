//! The `lakelands` binary: parses the command line and runs it, opening
//! lakes with [`lakelands_cli::open_store`].

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args_os().collect();
    let runtime = match lakelands_cli::runtime(&args) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: could not start: {e}");
            return ExitCode::FAILURE;
        }
    };
    // Unlocked handles: `serve --mcp` writes the protocol to standard output
    // from the server's own task, which a lock held here for the whole run
    // would block.
    let (mut out, mut err) = (std::io::stdout(), std::io::stderr());
    let code = runtime.block_on(lakelands_cli::run(args, &mut out, &mut err, &|l, m, s| {
        lakelands_cli::open_store(l, m, s)
    }));
    ExitCode::from(code)
}
