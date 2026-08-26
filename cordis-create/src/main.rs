use clap::Parser;
use cordis_create::{CreateCli, CreateOptions};
use std::io::{self, Write};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "cordis-create",
    version,
    about = "Create a new cordis-rs project"
)]
struct Arguments {
    /// Cargo package name.
    name: String,

    /// Output directory. Defaults to the package name.
    #[arg(short, long)]
    target: Option<PathBuf>,

    #[arg(long, default_value = "default")]
    template: String,

    #[arg(long = "ref")]
    ref_tag: Option<String>,

    #[arg(long)]
    git: bool,

    #[arg(long)]
    force: bool,

    #[arg(long)]
    mirror: Option<String>,

    #[arg(long)]
    prod: bool,

    #[arg(short = 'y', long)]
    yes: bool,
}

fn main() {
    let arguments = Arguments::parse();
    let target = arguments
        .target
        .clone()
        .unwrap_or_else(|| PathBuf::from(&arguments.name));
    if arguments.force && !arguments.yes && target_is_not_empty(&target) {
        print!("overwrite {}? [y/N] ", target.display());
        io::stdout().flush().expect("flush confirmation prompt");
        let mut answer = String::new();
        if io::stdin().read_line(&mut answer).is_err()
            || !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
        {
            eprintln!("cancelled");
            std::process::exit(2);
        }
    }
    let cli = CreateCli::new(CreateOptions {
        name: arguments.name,
        template: arguments.template,
        ref_tag: arguments.ref_tag,
        git: arguments.git,
        forced: arguments.force,
        mirror: arguments.mirror,
        prod: arguments.prod,
        yes: arguments.yes,
    });
    match cli.try_generate_template(&target) {
        Ok(path) => println!("created {}", path.display()),
        Err(error) => {
            eprintln!("error: {error}");
            std::process::exit(1);
        }
    }
}

fn target_is_not_empty(target: &std::path::Path) -> bool {
    target
        .read_dir()
        .ok()
        .and_then(|mut entries| entries.next())
        .is_some()
}
