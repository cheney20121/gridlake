# 第三方许可证

WebExcel 本身以 [Apache-2.0](LICENSE) 发布，并使用了下列开源软件。它们各自的许可证和版权声明以对应项目为准；
全部是宽松许可证（MIT / Apache-2.0 / BSD / ISC 等），没有 GPL / AGPL 等传染性许可证。

重点依赖：[Univer](https://github.com/dream-num/univer)（Apache-2.0，表格界面）、[DuckDB](https://github.com/duckdb/duckdb)（MIT，数据引擎）、
[Vue](https://github.com/vuejs/core)（MIT）、[Element Plus](https://github.com/element-plus/element-plus)（MIT）、[ECharts](https://github.com/apache/echarts)（Apache-2.0）、
[axum](https://github.com/tokio-rs/axum)（MIT）、[tokio](https://github.com/tokio-rs/tokio)（MIT）。

本文件由 `scripts/gen-third-party.py` 生成，依赖变化后请重新生成。

## 后端（Rust crates）（363 个）

| 名称 | 版本 | 许可证 |
|---|---|---|
| adler2 | 2.0.1 | 0BSD OR MIT OR Apache-2.0 |
| aead | 0.5.2 | MIT OR Apache-2.0 |
| aes | 0.8.4 | MIT OR Apache-2.0 |
| aes-gcm | 0.10.3 | Apache-2.0 OR MIT |
| ahash | 0.8.12 | MIT OR Apache-2.0 |
| aho-corasick | 1.1.5 | Unlicense OR MIT |
| allocator-api2 | 0.2.21 | MIT OR Apache-2.0 |
| android_system_properties | 0.1.6 | MIT OR Apache-2.0 |
| anyhow | 1.0.104 | MIT OR Apache-2.0 |
| arbitrary | 1.4.2 | MIT OR Apache-2.0 |
| arrow | 58.4.0 | Apache-2.0 |
| arrow-arith | 58.4.0 | Apache-2.0 |
| arrow-array | 58.4.0 | Apache-2.0 AND MIT |
| arrow-buffer | 58.4.0 | Apache-2.0 |
| arrow-cast | 58.4.0 | Apache-2.0 |
| arrow-data | 58.4.0 | Apache-2.0 |
| arrow-ord | 58.4.0 | Apache-2.0 |
| arrow-row | 58.4.0 | Apache-2.0 |
| arrow-schema | 58.4.0 | Apache-2.0 |
| arrow-select | 58.4.0 | Apache-2.0 |
| arrow-string | 58.4.0 | Apache-2.0 |
| async-compression | 0.4.50 | MIT OR Apache-2.0 |
| async-trait | 0.1.92 | MIT OR Apache-2.0 |
| atoi | 2.0.0 | MIT |
| atomic-waker | 1.1.2 | Apache-2.0 OR MIT |
| autocfg | 1.5.1 | Apache-2.0 OR MIT |
| axum | 0.8.9 | MIT |
| axum-core | 0.5.6 | MIT |
| base64 | 0.22.1 | MIT OR Apache-2.0 |
| base64 | 0.23.1 | MIT OR Apache-2.0 |
| base64ct | 1.8.3 | Apache-2.0 OR MIT |
| bitflags | 2.13.2 | MIT OR Apache-2.0 |
| block-buffer | 0.10.4 | MIT OR Apache-2.0 |
| bumpalo | 3.20.3 | MIT OR Apache-2.0 |
| byteorder | 1.5.0 | Unlicense OR MIT |
| bytes | 1.12.1 | MIT |
| calamine | 0.26.1 | MIT |
| cast | 0.3.0 | MIT OR Apache-2.0 |
| cc | 1.5.1 | MIT OR Apache-2.0 |
| cfg-if | 1.0.5 | MIT OR Apache-2.0 |
| cfg_aliases | 0.2.2 | MIT |
| chacha20 | 0.10.2 | MIT OR Apache-2.0 |
| chrono | 0.4.45 | MIT OR Apache-2.0 |
| cipher | 0.4.4 | MIT OR Apache-2.0 |
| codepage | 0.1.3 | Apache-2.0 OR MIT |
| comfy-table | 7.1.4 | MIT |
| compression-codecs | 0.4.45 | MIT OR Apache-2.0 |
| compression-core | 0.4.33 | MIT OR Apache-2.0 |
| const-oid | 0.9.6 | Apache-2.0 OR MIT |
| const-random | 0.1.18 | MIT OR Apache-2.0 |
| const-random-macro | 0.1.16 | MIT OR Apache-2.0 |
| core-foundation-sys | 0.8.7 | MIT OR Apache-2.0 |
| core_detect | 1.0.0 | MIT/Apache-2.0 |
| cpufeatures | 0.2.17 | MIT OR Apache-2.0 |
| cpufeatures | 0.3.1 | MIT OR Apache-2.0 |
| crc | 3.4.0 | MIT OR Apache-2.0 |
| crc-catalog | 2.5.0 | MIT OR Apache-2.0 |
| crc32fast | 1.5.2 | MIT OR Apache-2.0 |
| crossbeam-queue | 0.3.14 | MIT OR Apache-2.0 |
| crossbeam-utils | 0.8.23 | MIT OR Apache-2.0 |
| crossterm | 0.28.1 | MIT |
| crossterm_winapi | 0.9.1 | MIT |
| crunchy | 0.2.4 | MIT |
| crypto-common | 0.1.7 | MIT OR Apache-2.0 |
| csv | 1.4.0 | Unlicense/MIT |
| csv-core | 0.1.13 | Unlicense/MIT |
| ctr | 0.9.2 | MIT OR Apache-2.0 |
| der | 0.7.10 | Apache-2.0 OR MIT |
| derive_arbitrary | 1.4.2 | MIT OR Apache-2.0 |
| digest | 0.10.7 | MIT OR Apache-2.0 |
| displaydoc | 0.2.7 | MIT OR Apache-2.0 |
| dotenvy | 0.15.7 | MIT |
| duckdb | 1.10506.0 | MIT |
| either | 1.18.0 | MIT OR Apache-2.0 |
| email-encoding | 0.4.2 | MIT OR Apache-2.0 |
| email_address | 0.2.9 | MIT |
| encoding_rs | 0.8.42 | (Apache-2.0 OR MIT) AND BSD-3-Clause |
| equivalent | 1.0.2 | Apache-2.0 OR MIT |
| errno | 0.3.14 | MIT OR Apache-2.0 |
| etcetera | 0.8.0 | MIT OR Apache-2.0 |
| event-listener | 5.4.2 | Apache-2.0 OR MIT |
| fallible-iterator | 0.3.0 | MIT/Apache-2.0 |
| fallible-streaming-iterator | 0.1.9 | MIT/Apache-2.0 |
| fastrand | 2.5.0 | Apache-2.0 OR MIT |
| filetime | 0.2.29 | MIT/Apache-2.0 |
| find-msvc-tools | 0.1.14 | MIT OR Apache-2.0 |
| flate2 | 1.1.10 | MIT OR Apache-2.0 |
| foldhash | 0.1.5 | Zlib |
| form_urlencoded | 1.2.2 | MIT OR Apache-2.0 |
| futures-channel | 0.3.34 | MIT OR Apache-2.0 |
| futures-core | 0.3.34 | MIT OR Apache-2.0 |
| futures-intrusive | 0.5.0 | MIT OR Apache-2.0 |
| futures-io | 0.3.34 | MIT OR Apache-2.0 |
| futures-macro | 0.3.34 | MIT OR Apache-2.0 |
| futures-sink | 0.3.34 | MIT OR Apache-2.0 |
| futures-task | 0.3.34 | MIT OR Apache-2.0 |
| futures-util | 0.3.34 | MIT OR Apache-2.0 |
| generic-array | 0.14.7 | MIT |
| getrandom | 0.2.17 | MIT OR Apache-2.0 |
| getrandom | 0.3.4 | MIT OR Apache-2.0 |
| getrandom | 0.4.3 | MIT OR Apache-2.0 |
| ghash | 0.5.1 | Apache-2.0 OR MIT |
| half | 2.7.1 | MIT OR Apache-2.0 |
| hashbrown | 0.15.5 | MIT OR Apache-2.0 |
| hashbrown | 0.17.1 | MIT OR Apache-2.0 |
| hashlink | 0.10.0 | MIT OR Apache-2.0 |
| heck | 0.5.0 | MIT OR Apache-2.0 |
| hex | 0.4.3 | MIT OR Apache-2.0 |
| hkdf | 0.12.4 | MIT OR Apache-2.0 |
| hmac | 0.12.1 | MIT OR Apache-2.0 |
| home | 0.5.12 | MIT OR Apache-2.0 |
| hostname | 0.4.2 | MIT |
| http | 1.5.0 | MIT OR Apache-2.0 |
| http-body | 1.1.0 | MIT |
| http-body-util | 0.1.5 | MIT |
| http-range-header | 0.4.2 | MIT |
| httparse | 1.10.1 | MIT OR Apache-2.0 |
| httpdate | 1.0.3 | MIT OR Apache-2.0 |
| hyper | 1.11.1 | MIT |
| hyper-rustls | 0.27.10 | Apache-2.0 OR ISC OR MIT |
| hyper-util | 0.1.21 | MIT |
| iana-time-zone | 0.1.65 | MIT OR Apache-2.0 |
| iana-time-zone-haiku | 0.1.2 | MIT OR Apache-2.0 |
| icu_collections | 2.3.0 | Unicode-3.0 |
| icu_locale_core | 2.3.0 | Unicode-3.0 |
| icu_normalizer | 2.3.0 | Unicode-3.0 |
| icu_normalizer_data | 2.3.0 | Unicode-3.0 |
| icu_properties | 2.3.0 | Unicode-3.0 |
| icu_properties_data | 2.3.0 | Unicode-3.0 |
| icu_provider | 2.3.1 | Unicode-3.0 |
| idna | 1.1.0 | MIT OR Apache-2.0 |
| idna_adapter | 1.2.2 | Apache-2.0 OR MIT |
| indexmap | 2.14.2 | Apache-2.0 OR MIT |
| inout | 0.1.4 | MIT OR Apache-2.0 |
| ipnet | 2.12.2 | MIT OR Apache-2.0 |
| itoa | 1.0.18 | MIT OR Apache-2.0 |
| jobserver | 0.1.35 | MIT OR Apache-2.0 |
| js-sys | 0.3.106 | MIT OR Apache-2.0 |
| lazy_static | 1.5.1 | MIT OR Apache-2.0 |
| lettre | 0.11.23 | MIT |
| lexical-core | 1.0.6 | MIT/Apache-2.0 |
| lexical-parse-float | 1.0.6 | MIT/Apache-2.0 |
| lexical-parse-integer | 1.0.6 | MIT/Apache-2.0 |
| lexical-util | 1.0.7 | MIT/Apache-2.0 |
| lexical-write-float | 1.0.6 | MIT/Apache-2.0 |
| lexical-write-integer | 1.0.6 | MIT/Apache-2.0 |
| libc | 0.2.189 | MIT OR Apache-2.0 |
| libduckdb-sys | 1.10506.0 | MIT |
| libm | 0.2.16 | MIT |
| libredox | 0.1.25 | MIT |
| linux-raw-sys | 0.12.1 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| linux-raw-sys | 0.4.15 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| litemap | 0.8.3 | Unicode-3.0 |
| lock_api | 0.4.14 | MIT OR Apache-2.0 |
| log | 0.4.34 | MIT OR Apache-2.0 |
| lru-slab | 0.1.3 | MIT OR Apache-2.0 OR Zlib |
| matchers | 0.2.0 | MIT |
| matchit | 0.8.4 | MIT AND BSD-3-Clause |
| md-5 | 0.10.6 | MIT OR Apache-2.0 |
| memchr | 2.8.3 | Unlicense OR MIT |
| mime | 0.3.17 | MIT OR Apache-2.0 |
| mime_guess | 2.0.5 | MIT |
| miniz_oxide | 0.9.1 | MIT OR Zlib OR Apache-2.0 |
| mio | 1.2.3 | MIT |
| multer | 3.1.0 | MIT |
| multiversion_no_op | 1.0.0 | Apache-2.0 OR MIT |
| nom | 8.0.0 | MIT |
| nu-ansi-term | 0.50.3 | MIT |
| num-bigint | 0.4.8 | MIT OR Apache-2.0 |
| num-bigint-dig | 0.8.6 | MIT/Apache-2.0 |
| num-complex | 0.4.6 | MIT OR Apache-2.0 |
| num-integer | 0.1.47 | MIT OR Apache-2.0 |
| num-iter | 0.1.46 | MIT OR Apache-2.0 |
| num-traits | 0.2.19 | MIT OR Apache-2.0 |
| once_cell | 1.21.4 | MIT OR Apache-2.0 |
| opaque-debug | 0.3.1 | MIT OR Apache-2.0 |
| parking | 2.2.1 | Apache-2.0 OR MIT |
| parking_lot | 0.12.5 | MIT OR Apache-2.0 |
| parking_lot_core | 0.9.12 | MIT OR Apache-2.0 |
| pem-rfc7468 | 0.7.0 | Apache-2.0 OR MIT |
| percent-encoding | 2.3.2 | MIT OR Apache-2.0 |
| pin-project-lite | 0.2.17 | Apache-2.0 OR MIT |
| pkcs1 | 0.7.5 | Apache-2.0 OR MIT |
| pkcs8 | 0.10.2 | Apache-2.0 OR MIT |
| pkg-config | 0.3.34 | MIT OR Apache-2.0 |
| plain | 0.2.3 | MIT/Apache-2.0 |
| polyval | 0.6.2 | Apache-2.0 OR MIT |
| potential_utf | 0.1.6 | Unicode-3.0 |
| ppv-lite86 | 0.2.21 | MIT OR Apache-2.0 |
| proc-macro2 | 1.0.107 | MIT OR Apache-2.0 |
| quick-xml | 0.31.0 | MIT |
| quinn | 0.11.12 | MIT OR Apache-2.0 |
| quinn-proto | 0.11.19 | MIT OR Apache-2.0 |
| quinn-udp | 0.5.16 | MIT OR Apache-2.0 |
| quote | 1.0.47 | MIT OR Apache-2.0 |
| quoted_printable | 0.5.2 | 0BSD |
| r-efi | 5.3.0 | MIT OR Apache-2.0 OR LGPL-2.1-or-later |
| r-efi | 6.0.0 | MIT OR Apache-2.0 OR LGPL-2.1-or-later |
| rand | 0.10.3 | MIT OR Apache-2.0 |
| rand | 0.8.8 | MIT OR Apache-2.0 |
| rand_chacha | 0.3.1 | MIT OR Apache-2.0 |
| rand_core | 0.10.1 | MIT OR Apache-2.0 |
| rand_core | 0.6.4 | MIT OR Apache-2.0 |
| rand_pcg | 0.10.2 | MIT OR Apache-2.0 |
| redox_syscall | 0.5.18 | MIT |
| redox_syscall | 0.9.4 | MIT |
| regex | 1.13.1 | MIT OR Apache-2.0 |
| regex-automata | 0.4.18 | MIT OR Apache-2.0 |
| regex-syntax | 0.8.11 | MIT OR Apache-2.0 |
| reqwest | 0.12.28 | MIT OR Apache-2.0 |
| ring | 0.17.14 | Apache-2.0 AND ISC |
| rsa | 0.9.10 | MIT OR Apache-2.0 |
| rust_xlsxwriter | 0.99.1 | MIT OR Apache-2.0 |
| rustc-hash | 2.1.3 | Apache-2.0 OR MIT |
| rustix | 0.38.44 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| rustix | 1.1.5 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| rustls | 0.23.45 | Apache-2.0 OR ISC OR MIT |
| rustls-pki-types | 1.15.1 | MIT OR Apache-2.0 |
| rustls-webpki | 0.103.15 | ISC |
| rustversion | 1.0.23 | MIT OR Apache-2.0 |
| ryu | 1.0.23 | Apache-2.0 OR BSL-1.0 |
| scopeguard | 1.2.0 | MIT OR Apache-2.0 |
| serde | 1.0.229 | MIT OR Apache-2.0 |
| serde_core | 1.0.229 | MIT OR Apache-2.0 |
| serde_derive | 1.0.229 | MIT OR Apache-2.0 |
| serde_json | 1.0.151 | MIT OR Apache-2.0 |
| serde_path_to_error | 0.1.20 | MIT OR Apache-2.0 |
| serde_urlencoded | 0.7.1 | MIT/Apache-2.0 |
| sha1 | 0.10.7 | MIT OR Apache-2.0 |
| sha2 | 0.10.9 | MIT OR Apache-2.0 |
| sharded-slab | 0.1.7 | MIT |
| shlex | 2.0.1 | MIT OR Apache-2.0 |
| signal-hook-registry | 1.4.8 | MIT OR Apache-2.0 |
| signature | 2.2.0 | Apache-2.0 OR MIT |
| simd-adler32 | 0.3.10 | MIT |
| simdutf8 | 0.1.5 | MIT OR Apache-2.0 |
| slab | 0.4.12 | MIT |
| smallvec | 1.16.2 | MIT OR Apache-2.0 |
| socket2 | 0.6.5 | MIT OR Apache-2.0 |
| spin | 0.9.9 | MIT |
| spki | 0.7.3 | Apache-2.0 OR MIT |
| sqlx | 0.8.6 | MIT OR Apache-2.0 |
| sqlx-core | 0.8.6 | MIT OR Apache-2.0 |
| sqlx-macros | 0.8.6 | MIT OR Apache-2.0 |
| sqlx-macros-core | 0.8.6 | MIT OR Apache-2.0 |
| sqlx-mysql | 0.8.6 | MIT OR Apache-2.0 |
| sqlx-postgres | 0.8.6 | MIT OR Apache-2.0 |
| stable_deref_trait | 1.2.1 | MIT OR Apache-2.0 |
| stringprep | 0.1.5 | MIT/Apache-2.0 |
| strum | 0.27.2 | MIT |
| strum_macros | 0.27.2 | MIT |
| subtle | 2.6.1 | BSD-3-Clause |
| syn | 2.0.119 | MIT OR Apache-2.0 |
| syn | 3.0.6 | MIT OR Apache-2.0 |
| sync_wrapper | 1.0.2 | Apache-2.0 |
| synstructure | 0.14.0 | MIT |
| tar | 0.4.46 | MIT OR Apache-2.0 |
| tempfile | 3.27.0 | MIT OR Apache-2.0 |
| thiserror | 2.0.21 | MIT OR Apache-2.0 |
| thiserror-impl | 2.0.21 | MIT OR Apache-2.0 |
| thread_local | 1.1.10 | MIT OR Apache-2.0 |
| tiny-keccak | 2.0.2 | CC0-1.0 |
| tinystr | 0.8.4 | Unicode-3.0 |
| tinyvec | 1.13.3 | Zlib OR Apache-2.0 OR MIT |
| tokio | 1.53.1 | MIT |
| tokio-macros | 2.7.2 | MIT |
| tokio-rustls | 0.26.6 | MIT OR Apache-2.0 |
| tokio-stream | 0.1.19 | MIT |
| tokio-util | 0.7.19 | MIT |
| tower | 0.5.3 | MIT |
| tower-http | 0.6.11 | MIT |
| tower-layer | 0.3.3 | MIT |
| tower-service | 0.3.3 | MIT |
| tracing | 0.1.44 | MIT |
| tracing-attributes | 0.1.31 | MIT |
| tracing-core | 0.1.36 | MIT |
| tracing-log | 0.2.0 | MIT |
| tracing-subscriber | 0.3.23 | MIT |
| try-lock | 0.2.5 | MIT |
| typed-path | 0.12.3 | MIT OR Apache-2.0 |
| typenum | 1.20.1 | MIT OR Apache-2.0 |
| unicase | 2.9.0 | MIT OR Apache-2.0 |
| unicode-bidi | 0.3.18 | MIT OR Apache-2.0 |
| unicode-ident | 1.0.26 | (MIT OR Apache-2.0) AND Unicode-3.0 |
| unicode-normalization | 0.1.25 | MIT OR Apache-2.0 |
| unicode-properties | 0.1.4 | MIT/Apache-2.0 |
| unicode-segmentation | 1.13.3 | MIT OR Apache-2.0 |
| unicode-width | 0.2.2 | MIT OR Apache-2.0 |
| universal-hash | 0.5.1 | MIT OR Apache-2.0 |
| untrusted | 0.9.0 | ISC |
| ureq | 3.4.2 | MIT OR Apache-2.0 |
| ureq-proto | 0.6.4 | MIT OR Apache-2.0 |
| url | 2.5.8 | MIT OR Apache-2.0 |
| utf8-zero | 0.8.1 | MIT OR Apache-2.0 |
| utf8_iter | 1.0.4 | Apache-2.0 OR MIT |
| uuid | 1.27.0 | Apache-2.0 OR MIT |
| valuable | 0.1.1 | MIT |
| vcpkg | 0.2.15 | MIT/Apache-2.0 |
| version_check | 0.9.5 | MIT/Apache-2.0 |
| want | 0.3.1 | MIT |
| wasi | 0.11.1+wasi-snapshot-preview1 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| wasip2 | 1.0.4+wasi-0.2.12 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| wasite | 0.1.0 | Apache-2.0 OR BSL-1.0 OR MIT |
| wasm-bindgen | 0.2.129 | MIT OR Apache-2.0 |
| wasm-bindgen-futures | 0.4.79 | MIT OR Apache-2.0 |
| wasm-bindgen-macro | 0.2.129 | MIT OR Apache-2.0 |
| wasm-bindgen-macro-support | 0.2.129 | MIT OR Apache-2.0 |
| wasm-bindgen-shared | 0.2.129 | MIT OR Apache-2.0 |
| wasm-streams | 0.4.2 | MIT OR Apache-2.0 |
| web-sys | 0.3.106 | MIT OR Apache-2.0 |
| web-time | 1.1.0 | MIT OR Apache-2.0 |
| webpki-roots | 0.26.11 | CDLA-Permissive-2.0 |
| webpki-roots | 1.0.9 | CDLA-Permissive-2.0 |
| whoami | 1.6.1 | Apache-2.0 OR BSL-1.0 OR MIT |
| winapi | 0.3.9 | MIT/Apache-2.0 |
| winapi-i686-pc-windows-gnu | 0.4.0 | MIT/Apache-2.0 |
| winapi-x86_64-pc-windows-gnu | 0.4.0 | MIT/Apache-2.0 |
| windows-core | 0.62.2 | MIT OR Apache-2.0 |
| windows-implement | 0.60.2 | MIT OR Apache-2.0 |
| windows-interface | 0.59.3 | MIT OR Apache-2.0 |
| windows-link | 0.2.1 | MIT OR Apache-2.0 |
| windows-result | 0.4.1 | MIT OR Apache-2.0 |
| windows-strings | 0.5.1 | MIT OR Apache-2.0 |
| windows-sys | 0.48.0 | MIT OR Apache-2.0 |
| windows-sys | 0.52.0 | MIT OR Apache-2.0 |
| windows-sys | 0.59.0 | MIT OR Apache-2.0 |
| windows-sys | 0.61.2 | MIT OR Apache-2.0 |
| windows-targets | 0.48.5 | MIT OR Apache-2.0 |
| windows-targets | 0.52.6 | MIT OR Apache-2.0 |
| windows_aarch64_gnullvm | 0.48.5 | MIT OR Apache-2.0 |
| windows_aarch64_gnullvm | 0.52.6 | MIT OR Apache-2.0 |
| windows_aarch64_msvc | 0.48.5 | MIT OR Apache-2.0 |
| windows_aarch64_msvc | 0.52.6 | MIT OR Apache-2.0 |
| windows_i686_gnu | 0.48.5 | MIT OR Apache-2.0 |
| windows_i686_gnu | 0.52.6 | MIT OR Apache-2.0 |
| windows_i686_gnullvm | 0.52.6 | MIT OR Apache-2.0 |
| windows_i686_msvc | 0.48.5 | MIT OR Apache-2.0 |
| windows_i686_msvc | 0.52.6 | MIT OR Apache-2.0 |
| windows_x86_64_gnu | 0.48.5 | MIT OR Apache-2.0 |
| windows_x86_64_gnu | 0.52.6 | MIT OR Apache-2.0 |
| windows_x86_64_gnullvm | 0.48.5 | MIT OR Apache-2.0 |
| windows_x86_64_gnullvm | 0.52.6 | MIT OR Apache-2.0 |
| windows_x86_64_msvc | 0.48.5 | MIT OR Apache-2.0 |
| windows_x86_64_msvc | 0.52.6 | MIT OR Apache-2.0 |
| wit-bindgen | 0.57.1 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| writeable | 0.6.4 | Unicode-3.0 |
| xattr | 1.6.1 | MIT OR Apache-2.0 |
| yoke | 0.8.3 | Unicode-3.0 |
| yoke-derive | 0.8.4 | Unicode-3.0 |
| zerocopy | 0.8.59 | BSD-2-Clause OR Apache-2.0 OR MIT |
| zerocopy-derive | 0.8.59 | BSD-2-Clause OR Apache-2.0 OR MIT |
| zerofrom | 0.1.8 | Unicode-3.0 |
| zerofrom-derive | 0.1.8 | Unicode-3.0 |
| zeroize | 1.9.0 | Apache-2.0 OR MIT |
| zerotrie | 0.2.5 | Unicode-3.0 |
| zerovec | 0.11.8 | Unicode-3.0 |
| zerovec-derive | 0.11.6 | Unicode-3.0 |
| zip | 2.4.2 | MIT |
| zip | 6.0.0 | MIT |
| zip | 8.6.0 | MIT |
| zlib-rs | 0.6.8 | Zlib |
| zmij | 1.0.23 | MIT |
| zopfli | 0.8.3 | Apache-2.0 |

## 前端（npm 生产依赖）（339 个）

| 名称 | 版本 | 许可证 |
|---|---|---|
| @babel/helper-string-parser | 7.27.1 | MIT |
| @babel/helper-validator-identifier | 7.28.5 | MIT |
| @babel/parser | 7.28.6 | MIT |
| @babel/types | 7.28.6 | MIT |
| @ctrl/tinycolor | 3.6.1 | MIT |
| @element-plus/icons-vue | 2.3.2 | MIT |
| @fast-csv/format | 4.3.5 | MIT |
| @fast-csv/parse | 4.3.6 | MIT |
| @flatten-js/interval-tree | 1.1.3 | MIT |
| @floating-ui/core | 1.7.3 | MIT |
| @floating-ui/core | 1.8.0 | MIT |
| @floating-ui/dom | 1.7.4 | MIT |
| @floating-ui/dom | 1.8.0 | MIT |
| @floating-ui/react-dom | 2.1.9 | MIT |
| @floating-ui/utils | 0.2.10 | MIT |
| @floating-ui/utils | 0.2.12 | MIT |
| @grpc/grpc-js | 1.14.5 | Apache-2.0 |
| @grpc/proto-loader | 0.8.1 | Apache-2.0 |
| @interactjs/types | 1.10.27 | MIT |
| @jridgewell/sourcemap-codec | 1.5.5 | MIT |
| @js-sdsl/ordered-map | 4.4.2 | MIT |
| @juggle/resize-observer | 3.4.0 | Apache-2.0 |
| @protobufjs/aspromise | 1.1.2 | BSD-3-Clause |
| @protobufjs/base64 | 1.1.2 | BSD-3-Clause |
| @protobufjs/codegen | 2.0.5 | BSD-3-Clause |
| @protobufjs/eventemitter | 1.1.1 | BSD-3-Clause |
| @protobufjs/fetch | 1.1.1 | BSD-3-Clause |
| @protobufjs/float | 1.0.2 | BSD-3-Clause |
| @protobufjs/path | 1.1.2 | BSD-3-Clause |
| @protobufjs/pool | 1.1.0 | BSD-3-Clause |
| @protobufjs/utf8 | 1.1.2 | BSD-3-Clause |
| @radix-ui/primitive | 1.1.7 | MIT |
| @radix-ui/react-arrow | 1.1.15 | MIT |
| @radix-ui/react-collection | 1.1.15 | MIT |
| @radix-ui/react-compose-refs | 1.1.5 | MIT |
| @radix-ui/react-context | 1.2.2 | MIT |
| @radix-ui/react-dialog | 1.1.23 | MIT |
| @radix-ui/react-direction | 1.1.4 | MIT |
| @radix-ui/react-dismissable-layer | 1.1.19 | MIT |
| @radix-ui/react-dropdown-menu | 2.1.24 | MIT |
| @radix-ui/react-focus-guards | 1.1.6 | MIT |
| @radix-ui/react-focus-scope | 1.1.16 | MIT |
| @radix-ui/react-hover-card | 1.1.23 | MIT |
| @radix-ui/react-id | 1.1.4 | MIT |
| @radix-ui/react-menu | 2.1.24 | MIT |
| @radix-ui/react-popover | 1.1.23 | MIT |
| @radix-ui/react-popper | 1.3.7 | MIT |
| @radix-ui/react-portal | 1.1.17 | MIT |
| @radix-ui/react-presence | 1.1.10 | MIT |
| @radix-ui/react-primitive | 2.1.10 | MIT |
| @radix-ui/react-roving-focus | 1.1.19 | MIT |
| @radix-ui/react-separator | 1.1.15 | MIT |
| @radix-ui/react-slot | 1.3.3 | MIT |
| @radix-ui/react-use-callback-ref | 1.1.4 | MIT |
| @radix-ui/react-use-controllable-state | 1.2.6 | MIT |
| @radix-ui/react-use-effect-event | 0.0.5 | MIT |
| @radix-ui/react-use-is-hydrated | 0.1.3 | MIT |
| @radix-ui/react-use-layout-effect | 1.1.4 | MIT |
| @radix-ui/react-use-rect | 1.1.4 | MIT |
| @radix-ui/react-use-size | 1.1.4 | MIT |
| @radix-ui/rect | 1.1.3 | MIT |
| @sxzz/popperjs-es | 2.11.7 | MIT |
| @types/lodash | 4.17.23 | MIT |
| @types/lodash-es | 4.17.12 | MIT |
| @types/node | 14.18.63 | MIT |
| @types/node | 24.10.7 | MIT |
| @types/react | 19.3.0 | MIT |
| @types/web-bluetooth | 0.0.20 | MIT |
| @types/web-bluetooth | 0.0.21 | MIT |
| @univerjs/core | 1.0.2 | Apache-2.0 |
| @univerjs/data-validation | 1.0.2 | Apache-2.0 |
| @univerjs/design | 1.0.2 | Apache-2.0 |
| @univerjs/docs | 1.0.2 | Apache-2.0 |
| @univerjs/docs-drawing | 1.0.2 | Apache-2.0 |
| @univerjs/docs-ui | 1.0.2 | Apache-2.0 |
| @univerjs/drawing | 1.0.2 | Apache-2.0 |
| @univerjs/drawing-ui | 1.0.2 | Apache-2.0 |
| @univerjs/engine-formula | 1.0.2 | Apache-2.0 |
| @univerjs/engine-render | 1.0.2 | Apache-2.0 |
| @univerjs/find-replace | 1.0.2 | Apache-2.0 |
| @univerjs/icons | 1.43.1 | MIT |
| @univerjs/network | 1.0.2 | Apache-2.0 |
| @univerjs/preset-sheets-conditional-formatting | 1.0.2 | Apache-2.0 |
| @univerjs/preset-sheets-core | 1.0.2 | Apache-2.0 |
| @univerjs/preset-sheets-data-validation | 1.0.2 | Apache-2.0 |
| @univerjs/preset-sheets-drawing | 1.0.2 | Apache-2.0 |
| @univerjs/preset-sheets-filter | 1.0.2 | Apache-2.0 |
| @univerjs/preset-sheets-find-replace | 1.0.2 | Apache-2.0 |
| @univerjs/preset-sheets-hyper-link | 1.0.2 | Apache-2.0 |
| @univerjs/preset-sheets-note | 1.0.2 | Apache-2.0 |
| @univerjs/preset-sheets-sort | 1.0.2 | Apache-2.0 |
| @univerjs/preset-sheets-table | 1.0.2 | Apache-2.0 |
| @univerjs/presets | 1.0.2 | Apache-2.0 |
| @univerjs/protocol | 1.0.2 | Apache-2.0 |
| @univerjs/rpc | 1.0.2 | Apache-2.0 |
| @univerjs/sheets | 1.0.2 | Apache-2.0 |
| @univerjs/sheets-conditional-formatting | 1.0.2 | Apache-2.0 |
| @univerjs/sheets-conditional-formatting-ui | 1.0.2 | Apache-2.0 |
| @univerjs/sheets-data-validation | 1.0.2 | Apache-2.0 |
| @univerjs/sheets-data-validation-ui | 1.0.2 | Apache-2.0 |
| @univerjs/sheets-drawing | 1.0.2 | Apache-2.0 |
| @univerjs/sheets-drawing-ui | 1.0.2 | Apache-2.0 |
| @univerjs/sheets-filter | 1.0.2 | Apache-2.0 |
| @univerjs/sheets-filter-ui | 1.0.2 | Apache-2.0 |
| @univerjs/sheets-find-replace | 1.0.2 | Apache-2.0 |
| @univerjs/sheets-formula | 1.0.2 | Apache-2.0 |
| @univerjs/sheets-formula-ui | 1.0.2 | Apache-2.0 |
| @univerjs/sheets-hyper-link | 1.0.2 | Apache-2.0 |
| @univerjs/sheets-hyper-link-ui | 1.0.2 | Apache-2.0 |
| @univerjs/sheets-note | 1.0.2 | Apache-2.0 |
| @univerjs/sheets-note-ui | 1.0.2 | Apache-2.0 |
| @univerjs/sheets-numfmt | 1.0.2 | Apache-2.0 |
| @univerjs/sheets-numfmt-ui | 1.0.2 | Apache-2.0 |
| @univerjs/sheets-sort | 1.0.2 | Apache-2.0 |
| @univerjs/sheets-sort-ui | 1.0.2 | Apache-2.0 |
| @univerjs/sheets-table | 1.0.2 | Apache-2.0 |
| @univerjs/sheets-table-ui | 1.0.2 | Apache-2.0 |
| @univerjs/sheets-ui | 1.0.2 | Apache-2.0 |
| @univerjs/telemetry | 1.0.2 | Apache-2.0 |
| @univerjs/themes | 1.0.2 | Apache-2.0 |
| @univerjs/ui | 1.0.2 | Apache-2.0 |
| @unocss/reset | 66.5.12 | MIT |
| @vexip-ui/hooks | 2.9.4 | MIT |
| @vexip-ui/utils | 2.16.4 | MIT |
| @vue/compiler-core | 3.5.26 | MIT |
| @vue/compiler-dom | 3.5.26 | MIT |
| @vue/compiler-sfc | 3.5.26 | MIT |
| @vue/compiler-ssr | 3.5.26 | MIT |
| @vue/devtools-api | 6.6.4 | MIT |
| @vue/devtools-api | 7.7.9 | MIT |
| @vue/devtools-kit | 7.7.9 | MIT |
| @vue/devtools-shared | 7.7.9 | MIT |
| @vue/reactivity | 3.5.26 | MIT |
| @vue/runtime-core | 3.5.26 | MIT |
| @vue/runtime-dom | 3.5.26 | MIT |
| @vue/server-renderer | 3.5.26 | MIT |
| @vue/shared | 3.5.26 | MIT |
| @vueuse/core | 10.11.1 | MIT |
| @vueuse/core | 14.2.0 | MIT |
| @vueuse/metadata | 10.11.1 | MIT |
| @vueuse/metadata | 14.2.0 | MIT |
| @vueuse/shared | 10.11.1 | MIT |
| @vueuse/shared | 14.2.0 | MIT |
| @wendellhu/redi | 1.1.3 | MIT |
| ansi-regex | 5.0.1 | MIT |
| ansi-styles | 4.3.0 | MIT |
| archiver | 5.3.2 | MIT |
| archiver-utils | 2.1.0 | MIT |
| archiver-utils | 3.0.4 | MIT |
| aria-hidden | 1.2.6 | MIT |
| async | 3.2.6 | MIT |
| async-lock | 1.4.1 | MIT |
| async-validator | 4.2.5 | MIT |
| asynckit | 0.4.0 | MIT |
| axios | 1.13.2 | MIT |
| balanced-match | 1.0.2 | MIT |
| base64-js | 1.5.1 | MIT |
| big-integer | 1.6.52 | Unlicense |
| binary | 0.3.0 | MIT |
| birpc | 2.9.0 | MIT |
| bl | 4.1.0 | MIT |
| bluebird | 3.4.7 | MIT |
| brace-expansion | 1.1.12 | MIT |
| brace-expansion | 2.0.2 | MIT |
| buffer | 5.7.1 | MIT |
| buffer-crc32 | 0.2.13 | MIT |
| buffer-indexof-polyfill | 1.0.2 | MIT |
| buffers | 0.1.1 | MIT |
| call-bind-apply-helpers | 1.0.2 | MIT |
| chainsaw | 0.1.0 | MIT/X11 |
| class-variance-authority | 0.7.1 | Apache-2.0 |
| cliui | 8.0.1 | ISC |
| clsx | 2.1.1 | MIT |
| cmdk | 1.1.1 | MIT |
| collapse-white-space | 2.1.0 | MIT |
| color-convert | 2.0.1 | MIT |
| color-name | 1.1.4 | MIT |
| combined-stream | 1.0.8 | MIT |
| compress-commons | 4.1.2 | MIT |
| concat-map | 0.0.1 | MIT |
| copy-anything | 4.0.5 | MIT |
| core-util-is | 1.0.3 | MIT |
| crc-32 | 1.2.2 | Apache-2.0 |
| crc32-stream | 4.0.3 | MIT |
| csstype | 3.2.3 | MIT |
| dayjs | 1.11.23 | MIT |
| decimal.js | 10.6.0 | MIT |
| delayed-stream | 1.0.0 | MIT |
| detect-node-es | 1.1.0 | MIT |
| dunder-proto | 1.0.1 | MIT |
| duplexer2 | 0.1.4 | BSD-3-Clause |
| echarts | 5.6.0 | Apache-2.0 |
| element-plus | 2.13.1 | MIT |
| emoji-regex | 8.0.0 | MIT |
| end-of-stream | 1.4.5 | MIT |
| entities | 7.0.0 | BSD-2-Clause |
| es-define-property | 1.0.1 | MIT |
| es-errors | 1.3.0 | MIT |
| es-object-atoms | 1.1.1 | MIT |
| es-set-tostringtag | 2.1.0 | MIT |
| escalade | 3.2.0 | MIT |
| estree-walker | 2.0.2 | MIT |
| exceljs | 4.4.0 | MIT |
| fast-csv | 4.3.6 | MIT |
| fast-diff | 1.3.0 | Apache-2.0 |
| follow-redirects | 1.15.11 | MIT |
| form-data | 4.0.5 | MIT |
| franc-min | 6.2.0 | MIT |
| fs-constants | 1.0.0 | MIT |
| fs.realpath | 1.0.0 | ISC |
| fstream | 1.0.12 | ISC |
| function-bind | 1.1.2 | MIT |
| get-caller-file | 2.0.5 | ISC |
| get-intrinsic | 1.3.0 | MIT |
| get-nonce | 1.0.1 | MIT |
| get-proto | 1.0.1 | MIT |
| glob | 7.2.3 | ISC |
| gopd | 1.2.0 | MIT |
| graceful-fs | 4.2.11 | ISC |
| grid-layout-plus | 1.1.1 | MIT |
| has-symbols | 1.1.0 | MIT |
| has-tostringtag | 1.0.2 | MIT |
| hasown | 2.0.2 | MIT |
| hookable | 5.5.3 | MIT |
| html-to-image | 1.11.13 | MIT |
| ieee754 | 1.2.1 | BSD-3-Clause |
| immediate | 3.0.6 | MIT |
| inflight | 1.0.6 | ISC |
| inherits | 2.0.4 | ISC |
| interactjs | 1.10.27 | MIT |
| is-fullwidth-code-point | 3.0.0 | MIT |
| is-what | 5.5.0 | MIT |
| isarray | 1.0.0 | MIT |
| js-cookie | 3.0.5 | MIT |
| jszip | 3.10.2 | (MIT OR GPL-3.0-or-later) |
| kdbush | 4.1.0 | ISC |
| lazystream | 1.0.1 | MIT |
| lie | 3.3.0 | MIT |
| listenercount | 1.0.1 | ISC |
| lodash | 4.17.21 | MIT |
| lodash-es | 4.17.22 | MIT |
| lodash-es | 4.18.1 | MIT |
| lodash-unified | 1.0.3 | MIT |
| lodash.camelcase | 4.3.0 | MIT |
| lodash.defaults | 4.2.0 | MIT |
| lodash.difference | 4.5.0 | MIT |
| lodash.escaperegexp | 4.1.2 | MIT |
| lodash.flatten | 4.4.0 | MIT |
| lodash.groupby | 4.6.0 | MIT |
| lodash.isboolean | 3.0.3 | MIT |
| lodash.isequal | 4.5.0 | MIT |
| lodash.isfunction | 3.0.9 | MIT |
| lodash.isnil | 4.0.0 | MIT |
| lodash.isplainobject | 4.0.6 | MIT |
| lodash.isundefined | 3.0.1 | MIT |
| lodash.union | 4.6.0 | MIT |
| lodash.uniq | 4.5.0 | MIT |
| long | 5.3.2 | Apache-2.0 |
| magic-string | 0.30.21 | MIT |
| math-intrinsics | 1.1.0 | MIT |
| memoize-one | 6.0.0 | MIT |
| mime-db | 1.52.0 | MIT |
| mime-types | 2.1.35 | MIT |
| minimatch | 3.1.5 | ISC |
| minimatch | 5.1.6 | ISC |
| minimist | 1.2.8 | MIT |
| mitt | 3.0.1 | MIT |
| mkdirp | 0.5.6 | MIT |
| n-gram | 2.0.2 | MIT |
| nanoid | 3.3.11 | MIT |
| normalize-path | 3.0.0 | MIT |
| normalize-wheel-es | 1.2.0 | BSD-3-Clause |
| once | 1.4.0 | ISC |
| ot-json1 | 1.0.2 | ISC |
| ot-text-unicode | 4.0.0 | ISC |
| pako | 1.0.11 | (MIT AND Zlib) |
| path-is-absolute | 1.0.1 | MIT |
| perfect-debounce | 1.0.0 | MIT |
| picocolors | 1.1.1 | ISC |
| pinia | 3.0.4 | MIT |
| postcss | 8.5.6 | MIT |
| process-nextick-args | 2.0.1 | MIT |
| protobufjs | 7.6.6 | BSD-3-Clause |
| proxy-from-env | 1.1.0 | MIT |
| quickselect | 3.0.0 | ISC |
| rbush | 4.0.1 | MIT |
| react | 19.3.0 | MIT |
| react-dom | 19.3.0 | MIT |
| react-remove-scroll | 2.7.2 | MIT |
| react-remove-scroll-bar | 2.3.8 | MIT |
| react-style-singleton | 2.2.3 | MIT |
| readable-stream | 2.3.8 | MIT |
| readable-stream | 3.6.2 | MIT |
| readdir-glob | 1.1.3 | Apache-2.0 |
| require-directory | 2.1.1 | MIT |
| rfdc | 1.4.1 | MIT |
| rimraf | 2.7.1 | ISC |
| rxjs | 7.8.2 | Apache-2.0 |
| safe-buffer | 5.1.2 | MIT |
| safe-buffer | 5.2.1 | MIT |
| saxes | 5.0.1 | ISC |
| scheduler | 0.28.0 | MIT |
| setimmediate | 1.0.5 | MIT |
| sonner | 2.0.8 | MIT |
| sortablejs | 1.14.0 | MIT |
| source-map-js | 1.2.1 | BSD-3-Clause |
| speakingurl | 14.0.1 | BSD |
| string-width | 4.2.3 | MIT |
| string_decoder | 1.1.1 | MIT |
| string_decoder | 1.3.0 | MIT |
| strip-ansi | 6.0.1 | MIT |
| superjson | 2.2.6 | MIT |
| tar-stream | 2.2.0 | MIT |
| tmp | 0.2.7 | MIT |
| traverse | 0.3.9 | MIT/X11 |
| trigram-utils | 2.0.1 | MIT |
| tslib | 2.3.0 | 0BSD |
| tslib | 2.8.1 | 0BSD |
| typescript | 5.9.3 | Apache-2.0 |
| undici-types | 7.16.0 | MIT |
| unicount | 1.1.0 | ISC |
| unzipper | 0.10.14 | MIT |
| use-callback-ref | 1.3.3 | MIT |
| use-sidecar | 1.1.3 | MIT |
| util-deprecate | 1.0.2 | MIT |
| uuid | 8.3.2 | MIT |
| v-dropdown | 3.5.1 | MIT |
| vue | 3.5.26 | MIT |
| vue-demi | 0.14.10 | MIT |
| vue-router | 4.6.4 | MIT |
| vuedraggable | 4.1.0 | MIT |
| wrap-ansi | 7.0.0 | MIT |
| wrappy | 1.0.2 | ISC |
| xmlchars | 2.2.0 | MIT |
| y18n | 5.0.8 | ISC |
| yargs | 17.7.3 | MIT |
| yargs-parser | 21.1.1 | ISC |
| zip-stream | 4.1.1 | MIT |
| zrender | 5.6.1 | BSD-3-Clause |
