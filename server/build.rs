//! Makes a fresh clone build.
//!
//! `server/crates/api/src/embedded.rs` embeds `../web/dist` with `#[derive(RustEmbed)]`, and that directory is a build product:
//! gitignored, produced by `make build-web`. So `cargo build` in a clean checkout failed with
//! `folder '.../web/dist' does not exist` followed by two errors about a method the derive never generated -
//! three messages, none of which says "build the frontend first". `make setup` does not create it either, so
//! the documented onboarding path led straight into it, and the same failure is why a `git worktree` of this
//! repository could not be built.
//!
//! CI had been papering over it with a `mkdir -p web/dist && echo … > index.html` step duplicated in two jobs -
//! which is the evidence that the gap was known and only ever closed for machines.
//!
//! A placeholder is written, never a replacement: an existing `index.html` is left alone, so a real frontend
//! build is never clobbered. And it is a page that *says* what happened rather than an empty document, because
//! a blank UI at `/` sends someone looking for a routing bug.

use std::path::Path;

fn main() {
    let dist = Path::new("../web/dist");
    println!("cargo::rerun-if-changed=../web/dist");

    // Outside a checkout of this repository there is no frontend to stand in for, and creating directories in
    // someone else's tree is not this script's business.
    if !Path::new("../web").is_dir() {
        return;
    }
    let index = dist.join("index.html");
    if index.exists() {
        return;
    }
    // A failure here is **reported**, not swallowed. Swallowing it returns to the opaque three-error
    // `folder does not exist` this script exists to prevent - in a read-only checkout, precisely the case
    // where nobody can guess the remedy. One sentence naming the directory and the command is the whole point.
    let explain = |what: &str, error: &std::io::Error| -> String {
        format!(
            "cannot {what} ../web/dist: {error}\n\
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
         <p>This is a placeholder written by <code>server/build.rs</code> so that the server compiles in a\n\
         fresh checkout. The API is fully functional; only this page is missing.</p>\n\
         <p>Build the real UI with <code>make build-web</code>, or run it from Vite with\n\
         <code>make dev</code>.</p>\n\
         </body>\n\
         </html>\n",
    ) {
        panic!("{}", explain("write", &error));
    }
}
