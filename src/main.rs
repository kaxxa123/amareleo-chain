fn main() -> anyhow::Result<()> {
    let pkg_name = env!("CARGO_PKG_NAME");
    amareleo_chain::main_core(pkg_name, pkg_name)
}
