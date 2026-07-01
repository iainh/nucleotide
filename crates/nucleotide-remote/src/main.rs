// ABOUTME: Nucleotide remote workspace service binary
// ABOUTME: Runs protocol traffic over stdio in local, WSL, and SSH contexts

fn main() -> anyhow::Result<()> {
    nucleotide_remote::run_from_args(std::env::args().skip(1))
}
