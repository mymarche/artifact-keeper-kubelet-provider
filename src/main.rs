use std::io::{Read, Write};
use std::process::ExitCode;

use artifact_keeper_kubelet_provider::config::Config;
use artifact_keeper_kubelet_provider::error::Error;
use artifact_keeper_kubelet_provider::log::Logger;
use artifact_keeper_kubelet_provider::run;

/// A CredentialProviderRequest is a few KiB; anything past this is not one.
const MAX_STDIN: u64 = 1024 * 1024;

fn main() -> ExitCode {
    match try_main() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("ak-kubelet-provider: {e}");
            ExitCode::FAILURE
        }
    }
}

fn try_main() -> Result<(), Error> {
    let config = Config::from_env()?;
    let log = Logger::new(config.effective_level());

    let mut input = Vec::new();
    std::io::stdin()
        .take(MAX_STDIN)
        .read_to_end(&mut input)
        .map_err(|e| Error::Usage(format!("cannot read stdin: {e}")))?;

    let output = run(&config, &input, log)?;
    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(output.as_bytes())
        .and_then(|()| stdout.write_all(b"\n"))
        .and_then(|()| stdout.flush())
        .map_err(|e| Error::Usage(format!("cannot write stdout: {e}")))
}
