fn main() {
    // Pin the fluent style so Linux never falls back to the Qt style. The kit in ui/kit
    // draws every bordered control itself, so the style only affects plain text widgets.
    let config = slint_build::CompilerConfiguration::new().with_style("fluent".into());
    slint_build::compile_with_config("ui/app.slint", config).expect("compile ui/app.slint");
}
