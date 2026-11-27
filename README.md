# VIctory — RX 6600 Edition

An independent experimental fork focused on native Windows compatibility and lower memory overhead on modest AMD RDNA 2 systems.

**Development status: experimental.** The goal is correct 3D rendering and video/scene transitions at 1080p and a sustained 30 FPS, not a claim that these targets have already been achieved.

## Target hardware

| Component | Target |
| --- | --- |
| GPU | Radeon RX 6600, 8 GiB dedicated VRAM |
| CPU | Ryzen 5 5600G, 6 cores / 12 threads |
| System memory | 16 GiB |
| OS | Windows 11 x64 |
| Graphics | Native Direct3D 12 |
| Presentation | 1920 x 1080, 30 FPS target |

## Architecture

VIctory is a compatibility layer, not a full-system hardware emulator. This fork aims to minimize redundant translations, copies, resource allocations and shader compilation while preserving graphics correctness. Console and PC memory models are different even when the CPU and GPU families are similar.

## Development branches

- `a-publier`: inherited upstream source baseline; project documentation may differ.
- [`rx6600-optimization`](https://github.com/Lelicthepro/VIctory-Belobug-AMD-RX6600/tree/rx6600-optimization): experimental changes and regression tests.

## Implemented in the experimental branch

- Budget-aware retention of unused texture pool blocks: with an 8 GiB local budget, the optional reserve is capped at 1 GiB instead of 4 GiB. This is a policy limit, not measured savings.
- Automatic host-memory admission on systems with at most 16 GiB physical RAM; explicit configuration remains available.
- The original default GPU resource-retirement margin is retained when memory admission is automatic.
- On memory-limited hosts the automatic general pipeline worker count is capped at three; speculative compilation uses at most one worker when multiple workers are available. Dedicated screen-pass workers remain available.
- Regression cases cover budget thresholds and demand scheduling.

These changes have been committed, but Windows builds and physical RX 6600 tests have not yet been verified.

## Roadmap

1. Establish reproducible measurements with original, authorized test scenes: frame time, peak committed memory, DXGI local/non-local usage, dropped work and pipeline latency.
2. Reduce redundant shader conversions and validate cache correctness.
3. Verify vertex, pixel, compute and mesh shader output with known expected values; preserve required graphics work.
4. Verify scene composition, textures, depth, lighting, geometry and transitions with generated test assets.
5. Reduce CPU submission overhead and memory churn, then evaluate 30 FPS at 1080p.
6. Publish reproducible benchmark results and only promote changes that pass regression testing.

No frame-rate result, game compatibility claim or release date is guaranteed. Do not treat presentation rate as proof of correct rendering.

## Development and testing

Use the pinned Rust toolchain and the Windows MSVC build tools. Work with source code and self-authored or otherwise authorized fixtures only. A D3D12-capable Windows machine is needed for graphics integration testing.

```powershell
cargo test --locked -p victory-pipeline
cargo test --locked -p victory-gpu --features diagnostics
cargo build --locked --profile diag -p victory --features diagnostics
```

Keep the original graphics validation, host security policies and resource-lifetime safeguards enabled. Do not publish game binaries, assets, derived shader caches, personal logs, proprietary SDK material or credentials.

## Upstream documentation and credits

This is a derivative project, not an official release. The history and license of the original work are retained. See the [upstream README](https://github.com/Belobug/VIctory-Belobug-AMD/blob/a-publier/README.md) for its original measurements, technical notes, configuration and complete contributor acknowledgements.

Thanks to [Marvisak/VIctory](https://github.com/Marvisak/VIctory), [Belobug/VIctory-Belobug-AMD](https://github.com/Belobug/VIctory-Belobug-AMD), [fxceless/victory-fork](https://github.com/fxceless/victory-fork), [DexNom/VIctory](https://github.com/DexNom/VIctory), [FQY55kai/ZJOU-VICTORY](https://github.com/FQY55kai/ZJOU-VICTORY), [myokex/victory-amd](https://github.com/myokex/victory-amd) and all upstream contributors. Prior development includes AI-assisted work documented upstream. This fork also uses ChatGPT-assisted development.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE) and [asset policy](docs/asset-policy.md) for scope. Original notices remain in the source tree.
