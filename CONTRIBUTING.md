# Contributing

Thanks for helping out. A few things to know first.

## Ground rules

- **Rust only, no crates.** The app talks to AppKit and Core Graphics through `objc_msgSend`
  (the `msg!` macro in `src/gui.rs`). Please don't add dependencies.
- **Keep results the same.** Speed-ups to the converter must give bit-identical coordinates.
  Run `cargo test --release` and compare `g25app convert` output before and after.
- **No personal data.** Never commit a raw DNA file, or anything made from one.
- Comments are short and plain ASCII.

## Building and testing

```
cargo build --release
cargo test --release
cargo check --features check-gui     # type-checks the GUI on non-macOS machines
```

The qpAdm tests need a reference pack at `dist/reference.g25pack` and skip without it. See
the README for how to build one.

## Reporting bugs

Open an issue with your macOS version, which page you were on, and what you expected. If it's
about a raw DNA file, say which company and file format it came from, but don't attach the file.
