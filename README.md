### Keyfrek

Simple tool that captures all keypresses on X11 system and keeps a log of what shortcut, in what application was used and how many times.

Results are periodically saved to `capture.json` file in the app's folder.
To export the result, run `cargo run -- --export` which will create `report.xlsx` with full breakdown

For fun, this tool doesn't use any external tools, only X11 APIs.