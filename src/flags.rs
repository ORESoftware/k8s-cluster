pub fn apply_cli_flags() {
    let parser = match unsafe { flags2env::Flags2Env::load(None) } {
        Ok(parser) => parser,
        Err(error) => {
            eprintln!("shared-auth-mcp: flags-2-env unavailable; using environment only ({error})");
            return;
        }
    };
    let argv: Vec<String> = std::env::args().collect();
    match parser.parse(&argv, None) {
        Ok(overrides) => {
            for (key, value) in overrides {
                std::env::set_var(key, value);
            }
        }
        Err(error) => eprintln!("shared-auth-mcp: invalid CLI flags ({error})"),
    }
}
