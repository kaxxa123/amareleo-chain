fn main() -> anyhow::Result<()> {
    amareleo_chain::main_core(env!("CARGO_PKG_NAME"))
}
