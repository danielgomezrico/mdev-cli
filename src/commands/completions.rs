use clap::{Args, CommandFactory};
use clap_complete::{generate, Shell};

#[derive(Args)]
pub struct CompletionsArgs {
    /// Shell to generate completions for
    #[arg(value_enum)]
    pub shell: Shell,
}

pub fn run<C: CommandFactory>(args: &CompletionsArgs) -> i32 {
    let mut cmd = C::command();
    let bin_name = cmd.get_name().to_string();
    generate(args.shell, &mut cmd, bin_name, &mut std::io::stdout());
    0
}
