# Krit 0.2.0 golden artifacts

These artifacts were written by the Krit 0.2.0 release binary (`krit
--version` reports `Krit 0.2.0`), built from source commit `e1bff4f`
(`ci: preserve canonical sources in Windows packages`). Each case holds the
package exactly as that commit has it, plus the component and adjacent
metadata that binary wrote with:

```text
krit build --manifest krit.pkg --output artifact.wasm
```

| Case | Package source at `e1bff4f` | Artifact | Policy | World |
|---|---|---|---|---|
| `factorial` | `krit.pkg`, `examples/factorial.krit` | 1393 bytes, `blake3:6b3c8591…e8001b` | 1 | `krit:runtime/program@0.2.0` |
| `stateful-webhook` | `examples/stateful-webhook.krit.pkg` (as `krit.pkg`), `examples/stateful-webhook.krit`, `examples/stateful-webhook.host.json` (as `host.json`) | 5792 bytes, `blake3:a5c11286…7570fc` | 2 | `krit:runtime/webhook-state-program@0.2.0` |

`request.json`, `expect.status`, and `expect.stdout` were written for this
suite and confirmed against the same 0.2.0 binary with `krit sandbox` and
`krit invoke`.

Everything here is frozen. Do not rebuild, reformat, or edit these files: a
runtime that cannot load and run them has broken the artifact compatibility
window in `spec/WASM-SANDBOX.md`. Remove the directory only once 0.2 leaves
the window, which first happens for a 0.5 runtime.
