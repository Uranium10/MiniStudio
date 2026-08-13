# IPC 계약

Rust가 IPC 타입과 커맨드의 단일 원본이다. `specta::Type`과 `#[specta::specta]`를 통해 [bindings.ts](../src/engine/rust/bindings.ts)를 생성하며, 해당 파일은 직접 수정하지 않는다.

## 타입 재생성

Rust 커맨드 인자·반환값·직렬화 필드를 바꾼 뒤 저장소 루트에서 아래를 실행한다.

```bash
npm run bindings
npm run build
```

첫 명령은 `export_bindings` Rust 테스트를 실행해 `src/engine/rust/bindings.ts`를 덮어쓴다. 생성 파일은 커밋한다. `u64`/`usize`는 플레이헤드·revision·프레임 카운터가 JavaScript 안전 정수 범위에 머문다는 엔진 불변식 아래 `number`로 생성한다.

바인딩 테스트는 Tauri GUI 런타임과 분리된 `--no-default-features`/`MockRuntime` 구성을 사용한다. `package.json` 스크립트가 이 플래그를 포함하므로 직접 Cargo를 실행할 때도 `cargo test --no-default-features`를 사용한다.

## 커맨드

| 커맨드 | 인자 | 반환 | 방식 / 빈도 |
|---|---|---|---|
| `engine_init`, `engine_dispose` | 없음 | `void` | 앱 수명당 1회 |
| `engine_load_audio_file` | path, `Channel<DecodeProgress>` | `NativeAssetInfo` | 비동기, 파일 로드 시 |
| `engine_unload_asset` | assetId | `void` | 에셋 제거 시 |
| `engine_sync_graph` | `GraphSnapshot` | `void` | 구조 변경 시만 |
| `engine_play/pause/stop/seek` | transport 값 | `void` | 사용자 조작 시 |
| `engine_set_track_*` | trackId, 값 | `void` | 실시간 파라미터 조작 |
| `engine_set_send_level` | sendId, gainDb | `void` | 실시간 파라미터 조작 |
| `engine_set_effect_param` | effectId, paramId, 값 | `void` | 실시간 파라미터 조작 |
| `engine_poll_state` | 없음 | `EngineSnapshot` | 재생 30Hz, 정지 4Hz |
| `engine_list_audio_backends` | 없음 | `AudioBackendInfo[]` | 설정 창 진입 시 |
| `engine_list_output_devices` | backendId | `AudioDeviceInfo[]` | 백엔드 변경 시 |
| `engine_get/set_audio_settings` | 설정 | 설정 또는 `void` | 설정 창/적용 시 |
| `engine_export_project` | request, `Channel<ExportProgress>` | `ExportResult` | 비동기 작업당 1회 |
| `engine_cancel_export` | 없음 | `void` | 취소 요청 시 |
| `engine_eq_response` | effectId, points | `EqFrequencyResponse` | EQ UI 갱신 시 |
| `scan_vst3_plugins`, `render_offline_effect` | 3단계 타입 | 3단계 타입 | 현재 스텁 |

모든 typed 커맨드는 `Result<T, EngineError>`다. UI는 `EngineError.kind`로 `DeviceNotFound`, `UnsupportedFormat`, `QueueFull`, `Stream`, `Asset`, `InvalidRequest`, `Internal`을 구분한다.

## Channel

- 디코딩: `DecodeProgress { stage, fraction }`
- 익스포트: `ExportProgress { stage, renderedFrames, totalFrames, fraction }`

Channel은 호출에 귀속되고 호출 종료와 함께 정리된다. 작업 진행률에는 전역 이벤트를 사용하지 않는다.

## 바이너리 IPC

`binary.ts`만 생성 바인딩의 예외다. `plugin:binary|get_asset_peaks`는 Tauri `ipc::Response`를 통해 원시 바이트를 반환한다.

- 바이트 순서: little-endian
- 원소: IEEE-754 `f32`
- 배열: `[min0, max0, min1, max1, ...]`
- LOD 버킷: 256 / 1024 / 4096 / 16384 samples

Rust는 `bytemuck::cast_slice`로 `&[f32]`를 바이트 뷰로 변환하고 TS는 `Float32Array`로 읽는다. base64와 JSON 숫자 배열은 사용하지 않는다.

## 폴링과 이벤트

폴링 페이로드는 트랙 ID가 없는 그래프 순서 `Level[]`과 `graphRevision`을 사용한다. TS는 그래프 동기화 때만 ID 배열을 만들고 revision 변경 시 교체한다. f32 미터를 i16 0.1dB 고정소수점으로 바꾸는 최적화는 프로파일링에서 병목이 확인될 때만 적용한다.

현재 전역 이벤트는 없다. 향후 디바이스 분리, 복구 불가능한 스트림 오류, xrun 임계 초과처럼 특정 호출에 귀속되지 않는 알림만 전역 이벤트로 추가한다.
