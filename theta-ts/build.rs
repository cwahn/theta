use cfg_aliases::cfg_aliases;

fn main() {
    cfg_aliases! {
        wasm_browser: { all(target_family = "wasm", target_os = "unknown") },
    }
}
