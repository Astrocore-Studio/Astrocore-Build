use std::io::{self, Write};
use std::path::Path;
use std::process::ExitCode;

use astrocore_build_2d::convert_ss14_catalog;

fn main() -> ExitCode {
    if let Err(error) = run() {
        eprintln!("{error}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let [yaml_directory, selection] = args.as_slice() else {
        return Err("astrocore-2d-import-prototypes <каталог YAML> <список импорта RON>".into());
    };
    let selection = Path::new(selection);
    let source = std::fs::read_to_string(selection)
        .map_err(|error| format!("{}: {error}", selection.display()))?;
    let result = convert_ss14_catalog(Path::new(yaml_directory), &source)?;
    writeln!(io::stdout().lock(), "{result}").map_err(|error| error.to_string())
}
