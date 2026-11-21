fn main() {
    // Development fixtures inspect actual row geometry without UI debug properties.
    slint_build::compile_with_config(
        "ui/app.slint",
        slint_build::CompilerConfiguration::new().with_debug_info(cfg!(debug_assertions)),
    )
    .expect("failed to compile Slint UI");
}
