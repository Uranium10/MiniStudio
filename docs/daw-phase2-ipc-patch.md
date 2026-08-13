# 2단계 보정 지시 — IPC 계층 개선

> 진행 중인 작업에 추가로 적용할 지시입니다. 현재 작업을 중단하지 말고, 아래 항목을 반영하세요.
> 이미 구현이 끝난 부분이 있다면 해당 방식으로 리팩터링하고, 아직 안 만든 부분은 처음부터 이 방식으로 작성하세요.

---

## 1. Rust ↔ TypeScript 타입 자동 생성 (최우선)

**문제**: Rust 구조체와 TS 인터페이스를 수동으로 이중 선언하면, 필드명이나 타입이 어긋나도 양쪽 다 컴파일이 통과하고 런타임 역직렬화 에러로만 드러난다. 커맨드가 20개 이상 생기는 이 프로젝트에서는 반드시 사고가 난다.

**해결**: `tauri-specta`(+ `specta`)를 도입해 Rust에서 TS 바인딩을 자동 생성한다.

- IPC 경계를 넘나드는 **모든 타입**에 `#[derive(Serialize, Deserialize, specta::Type)]`를 붙인다.
- 모든 `#[tauri::command]`에 `#[specta::specta]`를 함께 붙인다.
- 빌드 시 `src/engine/rust/bindings.ts`로 타입 붙은 `invoke` 래퍼를 생성한다.
- `RustEngine.ts`는 **생성된 바인딩만 사용**한다. `invoke('command_name', ...)`를 문자열로 직접 호출하는 코드가 남아 있으면 안 된다.
- 생성 파일은 git에 커밋하되 상단에 "자동 생성 — 직접 수정 금지" 주석을 넣는다.
- `package.json`에 바인딩 재생성 스크립트를 추가하고, README에 "Rust 커맨드 시그니처를 바꾸면 반드시 재생성"을 명시한다.

이걸 넣으면 Rust에서 파라미터 이름이나 반환 타입을 바꿨을 때 **TS 컴파일 단계에서 즉시 에러**가 난다. 이것이 이 보정의 핵심 목적이다.

## 2. 바이너리 데이터는 base64 대신 `ipc::Response`

**문제**: 기존 지시에서 파형 피크를 base64로 인코딩해 전송하라고 했으나, 이는 Tauri v1 시절의 우회책이다. v2는 원시 바이트 전송을 지원한다.

**해결**: 피크 데이터 등 대용량 바이너리는 `tauri::ipc::Response`로 반환한다.

```rust
#[tauri::command]
fn get_asset_peaks(state: State<EngineState>, asset_id: String, lod: u8)
    -> Result<tauri::ipc::Response, String>
{
    let peaks: &[f32] = /* 해당 LOD의 min/max 인터리브 배열 */;
    Ok(tauri::ipc::Response::new(bytemuck::cast_slice(peaks).to_vec()))
}
```

```ts
const buf = await invoke<ArrayBuffer>('get_asset_peaks', { assetId, lod });
const peaks = new Float32Array(buf);
```

- base64 인코딩/디코딩 코드는 전부 제거한다.
- 엔디안은 리틀엔디안으로 고정하고 문서화한다.
- `bytemuck`을 의존성에 추가한다 (`cast_slice`가 안전한 변환을 보장).
- 피크 외에도 향후 대용량 전송이 생기면 같은 경로를 쓴다.

주의: `ipc::Response`는 specta 타입 생성 대상이 아니므로, 이 커맨드들만은 반환 타입을 TS에서 수동으로 `ArrayBuffer`로 선언한다. 해당 함수들을 `src/engine/rust/binary.ts`에 모아두고, 바이트 레이아웃을 주석으로 명시한다.

## 3. 익스포트 진행률은 이벤트 대신 `Channel`

**문제**: 전역 이벤트(`emit`/`listen`)는 앱 전역에 브로드캐스트되므로, 특정 작업에 국한된 스트리밍에는 과하다. 리스너 해제 누락 시 누수도 생긴다.

**해결**: 오프라인 익스포트 진행률은 `tauri::ipc::Channel<T>`로 전달한다.

```rust
#[tauri::command]
async fn export_wav(
    state: State<'_, EngineState>,
    req: ExportRequest,
    on_progress: tauri::ipc::Channel<ExportProgress>,
) -> Result<ExportResult, String> {
    // 렌더 루프 중 on_progress.send(ExportProgress { .. })?;
}
```

```ts
const ch = new Channel<ExportProgress>();
ch.onmessage = (p) => store.setExportProgress(p);
const result = await invoke('export_wav', { req, onProgress: ch });
```

- 채널은 그 호출에만 묶이므로 작업 종료 시 자동 정리된다.
- 에셋 디코딩 진행률(긴 파일)도 같은 방식으로 전환한다.
- **전역 이벤트는 "특정 호출에 속하지 않는 알림"에만 남긴다**: 디바이스 분리, 스트림 오류, xrun 임계 초과 등.

## 4. 폴링 커맨드 최적화

`poll_engine_state`는 초당 30회 호출되므로 다른 커맨드보다 비용에 민감하다.

- 반환 구조체를 **평탄한 고정 크기**로 유지한다. `HashMap<String, Meter>`처럼 트랙 ID 문자열을 키로 쓰지 말고, **그래프 인덱스 순서의 배열**로 반환한다. 문자열 할당과 해시가 매 프레임 발생하는 것을 막는다.
  - TS 쪽에서 인덱스 ↔ 트랙 ID 매핑을 유지하고, 그래프 변경 시에만 갱신한다.
- 미터 값은 f32 대신 **i16 (0.1dB 단위 고정소수점)** 으로 보내도 충분하다. JSON 숫자 크기가 줄어든다. 다만 가독성을 해치므로, 실측 후 병목이 확인될 때만 적용한다. 우선은 f32로 두고 프로파일링 항목으로 기록만 남긴다.
- 반환 구조체에 `graph_revision: u64`를 포함시킨다. TS는 이 값이 바뀌었을 때만 인덱스 매핑을 다시 계산한다.
- 재생 중이 아닐 때는 폴링 주기를 4Hz로 낮춘다.

## 5. 커맨드 에러 타입 정비

`Result<T, String>`으로 통일하되, 에러 문자열을 즉석에서 `format!`하지 말고 전용 에러 열거형을 정의한 뒤 `Display`로 변환한다.

```rust
#[derive(Debug, thiserror::Error, Serialize, specta::Type)]
#[serde(tag = "kind", content = "detail")]
enum EngineError {
    #[error("device not found: {0}")]
    DeviceNotFound(String),
    #[error("unsupported format: {0}")]
    UnsupportedFormat(String),
    #[error("command queue full")]
    QueueFull,
    #[error("stream error: {0}")]
    Stream(String),
    // ...
}
```

- 커맨드 반환 타입은 `Result<T, EngineError>`로 하고, specta가 TS 유니온 타입을 생성하게 한다.
- UI는 `kind`로 분기해 사용자에게 적절한 안내를 띄운다 (예: `DeviceNotFound` → 오디오 설정 다이얼로그 열기 제안).
- `unwrap()` / `expect()`는 초기화 경로에서도 지양하고, 실패를 UI로 올린다.

## 6. IPC 계층 문서화

`docs/ipc-contract.md`를 작성한다.

- 전체 커맨드 목록: 이름, 인자, 반환 타입, 동기/비동기, 호출 빈도
- 이벤트 목록과 페이로드
- Channel 사용처
- 바이너리 페이로드의 바이트 레이아웃 (피크 배열 등)
- **타입 재생성 절차** — Rust 시그니처 변경 시 무엇을 실행해야 하는지

---

## 우선순위

1번(타입 자동 생성)이 가장 중요하다. 나머지는 성능·구조 개선이지만, 1번은 **버그를 원천 차단하는 안전장치**다. 다른 작업보다 먼저 반영하고, 이후 작성하는 모든 커맨드가 이 체계를 따르게 한다.

2번은 파형 표시 구현 전에 반영해야 나중에 뜯어고칠 일이 없다.

3~5번은 해당 기능을 구현하는 시점에 적용하면 된다.
