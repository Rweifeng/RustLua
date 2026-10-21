# paste compatibility shim

The archived `paste 1.0.15` crate is not used by this repository. This package
contains the maintained `pastey 0.2.3` proc-macro implementation, packaged under
the name/version `paste 1.0.15` so unmodified dependencies that declare
`paste ^1.0` (currently `egui_dock`, `simba`, `metal`) resolve to it through
`[patch.crates-io]`.

Why the package-name alias:

- Cargo cannot patch a dependency named `paste` with a path package named
  `pastey`; `package = "pastey"` is only a rename hint and is rejected/ignored
  for this same-version dependency edge.
- Renaming the package keeps the dependency graph free of the archived
  `paste` crate while preserving the macro surface (`paste!` and `item!`)
  expected by those crates.
- This is **not** the unmaintained `paste` source. The vendored code is
  `pastey 0.2.3` (LICENSE-MIT / LICENSE-APACHE in this directory); the
  `RUSTSEC-2024-0436` advisory applies to the archived crate, not this code.

Maintenance:

- Keep `baseVersion`/lock at `1.0.15` (the compatibility version exposed to
  dependents) and document the real upstream (`pastey 0.2.3`) in
  `engine/vendor/FORKS.json`.
- When a dependent stops requiring `paste`, remove this shim and let it depend
  on `pastey` directly.
- `cargo tree -i paste --target all` must point only at this path package; if a
  registry `paste` reappears, `cargo audit` will flag it again.
