# sena-rs

<img src="./assets/icon.png" alt="icon" align="left" width="100" style="margin-right: 10px;" />

Rust cross-platform implementation of the SoftPAL engine.
Specifically, targeted for the game "Koikake".

<br clear="left"/>

## Example screenshots

| Platform | Targets |
|---|---|
| Linux | x86_64, aarch64 |
| Windows | x86_64, ARM64 |
| macOS | DMG app bundle |
| iOS | arm64 device, arm64 simulator, x86_64 simulator |
| Android | arm64-v8a, x86_64 |
| WebAssembly | wasm32-unknown-unknown |

## Run

```bash
cargo run -p pal-vm --release --bin sena -- /path/to/game --nls sjis
```

## License
This project is licensed under the MPL-2.0 License. See [LICENSE](./LICENSE-MPL-2.0) for details.
