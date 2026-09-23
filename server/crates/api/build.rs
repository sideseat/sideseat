//! Ensures the frontend embed has an input directory in a fresh checkout.

use std::path::Path;

fn main() {
    let web = Path::new("../../../web");
    let dist = web.join("dist");
    println!("cargo::rerun-if-changed=../../../web/dist");

    if !web.is_dir() {
        return;
    }
    let index = dist.join("index.html");
    if index.exists() {
        return;
    }
    let explain = |what: &str, error: &std::io::Error| -> String {
        format!(
            "cannot {what} web/dist: {error}\n\
             The server embeds that directory, so it must exist to compile. Either make the checkout \
             writable, or build the frontend with `make build-web`."
        )
    };
    if let Err(error) = std::fs::create_dir_all(dist) {
        panic!("{}", explain("create", &error));
    }
    if let Err(error) = std::fs::write(
        &index,
        "<!doctype html>\n\
         <html lang=\"en\">\n\
         <head><meta charset=\"utf-8\"><title>SideSeat - UI not built</title></head>\n\
         <body style=\"font:16px/1.5 system-ui;max-width:40em;margin:4em auto;padding:0 1em\">\n\
         <h1>The workbench UI has not been built</h1>\n\
         <p>This is a placeholder written by the <code>sideseat-api</code> build script so that the server compiles in a\n\
         fresh checkout. The API is fully functional; only this page is missing.</p>\n\
         <p>Build the real UI with <code>make build-web</code>, or run it from Vite with\n\
         <code>make dev</code>.</p>\n\
         </body>\n\
         </html>\n",
    ) {
        panic!("{}", explain("write", &error));
    }
}
