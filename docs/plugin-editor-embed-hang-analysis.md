# 악기/이펙트 호스팅 창 "무한 로딩" 조사 — 임베디드 에디터 스레드 위반

> 분석 + 구체적 수정 방향 문서. 코드 변경 없음. 실기 재생/GUI 임베딩을 직접 검증할 수 없는
> 환경이라 정적 분석 + 벤더링된 크레이트 문서 확인으로 원인을 특정함. 신뢰도 높음(아래 근거 참조).

## 증상

악기(또는 이펙트) 호스팅 창을 열면 툴바(오토메이션 버튼, 프리셋 SAVE/LOAD)는 뜨는데,
`.plugin-shell-native-surface`의 "네이티브 플러그인 화면을 연결하는 중…" 플레이스홀더가
실제 플러그인 GUI로 절대 교체되지 않고 그대로 멈춤.

## 근본 원인 (확정) — `EmbeddedEditor::embed`를 잘못된 스레드에서 호출함

벤더링된 크레이트 `src-tauri/vendor/vst3-host/src/embed.rs`의 `EmbeddedEditor::embed` 문서에
**명시적으로** 이렇게 적혀 있음 ([embed.rs:78](../src-tauri/vendor/vst3-host/src/embed.rs#L78)):

> "Must be called on the UI/main thread (where your event loop runs)."

즉 `embed()`(그리고 사실상 `try_set_rect`/`set_rect`로 이어지는 리사이즈, 그리고 `EmbeddedEditor`의
`Drop`으로 이어지는 종료까지)는 **`parent` 윈도우를 소유한 스레드**, 즉 여기서는 Tauri/WebView2
메인 스레드에서 호출해야 함.

그런데 실제 호출 경로는 그 반대임:

1. **[lib.rs:719-734](../src-tauri/src/lib.rs#L719-L734), [lib.rs:808-822](../src-tauri/src/lib.rs#L808-L822)**
   — `engine_open_plugin_editor` Tauri 커맨드가 `tauri::async_runtime::spawn_blocking(...)`으로
   **tokio 블로킹 스레드풀**에서 `open_plugin_editor_embedded(...)`를 호출함 (메인 스레드 아님).
2. 그 안에서 `PluginControl::open_editor_embedded` → `Vst3Control::request(...)`
   ([lib.rs:560-573](../src-tauri/crates/ministudio-plugin/src/lib.rs#L560-L573))가
   메시지를 채널로 보내서, **`ministudio-vst3-control`이라는 완전히 별도의 전용 스레드**
   ([lib.rs:517-527](../src-tauri/crates/ministudio-plugin/src/lib.rs#L517-L527)에서
   `std::thread::Builder::new().spawn(...)`로 기동)에서 실행되는
   `run_vst3_control`의 `OpenEmbedded` 분기
   ([lib.rs:621-647](../src-tauri/crates/ministudio-plugin/src/lib.rs#L621-L647))가
   **실제로 `EmbeddedEditor::embed(...)`를 호출함**.

정리하면 **"메인 스레드에서 호출해야 하는 API"를, "메인 스레드도 아니고 spawn_blocking 스레드도
아닌, 완전히 별도의 제3의 전용 스레드"에서 호출**하고 있음. 2단계로 어긋나 있는 것.

CLAP도 완전히 동일한 구조로 같은 문제를 가짐 — `ClapControl`
([lib.rs:1270-1308](../src-tauri/crates/ministudio-plugin/src/lib.rs#L1270-L1308))도
자체 전용 스레드 + 커맨드 채널로 `open_editor_embedded`를 처리함. **VST3/CLAP 둘 다 고쳐야 함.**

리사이즈도 같은 문제: `WindowEvent::Resized` 핸들러
([lib.rs:765-778](../src-tauri/src/lib.rs#L765-L778))는 `resize_plugin_editor`를 호출하는데,
이건 그냥 `pending_editor_rect`에 값을 써두기만 하고
([lib.rs:574-578](../src-tauri/crates/ministudio-plugin/src/lib.rs#L574-L578)), 실제
`editor.set_rect(rect)` 적용은 다시 `run_vst3_control` 백그라운드 스레드의 폴링 루프
([lib.rs:680-686](../src-tauri/crates/ministudio-plugin/src/lib.rs#L680-L686))에서 일어남 —
역시 메인 스레드가 아님.

### 왜 "완전히 무한"이 아니라 10초쯤 뒤에 에러가 나야 정상인데?

`Vst3Control::request`/`ClapControl::request`는 `recv_timeout(Duration::from_secs(10))`로
호출자 쪽에서 타임아웃을 검사함 ([lib.rs:544-546](../src-tauri/crates/ministudio-plugin/src/lib.rs#L544-L546)).
그래서 이론상으로는 완전한 데드락이어도 10초 뒤엔 `"VST3 control operation timed out"` 에러가
나고, `engine_open_plugin_editor`의 폴백 로직
([lib.rs:824-836](../src-tauri/src/lib.rs#L824-L836))이 독립창(standalone) 방식으로 재시도함
(근데 그것도 같은 `Vst3Control`/`ClapControl` 스레드를 다시 쓰므로 마찬가지로 10초 더 걸릴 수 있음).

**그런데 이 에러가 나더라도 `PluginEditorShell.tsx`의 플레이스홀더 텍스트는 절대 안 바뀜.**
[PluginEditorShell.tsx:43](../src/components/PluginEditorShell.tsx#L43)를 보면:

```tsx
<section className="plugin-shell-native-surface"><div><span />네이티브 플러그인 화면을 연결하는 중…</div></section>
```

이 문구는 **`state`나 성공/실패 여부와 전혀 무관하게 항상 렌더링됨.** 임베딩이 성공하면 실제
네이티브 창이 그 위를 덮어서 안 보이게 되는 방식으로 설계된 것으로 보이는데, 실패하면 (에러 토스트는
메인 창에 뜨더라도) 이 서브 창 자체는 아무 피드백 없이 그 문구를 계속 띄운 채로 남음 →
사용자 입장에선 "10초짜리 타임아웃"이 아니라 그냥 "무한 로딩"으로 보임.

## 수정 방향

### 1) 임베딩 관련 호출을 전부 Tauri 메인 스레드로 옮기기 (핵심)

이 Tauri 버전(2.11.5)은 정확히 이 용도의 API를 제공함:
`AppHandle`/`WebviewWindow`/`Window` 모두에 있는
`run_on_main_thread<F: FnOnce() + Send + 'static>(&self, f: F) -> Result<()>`
(`tauri-2.11.5/src/app.rs:495` 등, 로컬 cargo 레지스트리 캐시에서 확인).

권장 구조:

- **임베디드 경로(`open_editor_embedded`/`resize_editor`/임베디드 종료)는 `Vst3Control`/
  `ClapControl`의 기존 커맨드 채널·전용 스레드에서 완전히 분리**한다. 독립창(standalone)
  경로(`Open`/`Close`/`SaveState`/`LoadState`)는 자기 소유의 별도 최상위 창을 여는 것이라
  전용 스레드에 남겨둬도 무방함 — 문제는 임베디드 경로만.
- `engine_open_plugin_editor` 커맨드에서: `tauri::async_runtime::spawn_blocking(...)` 대신
  `app.run_on_main_thread(move || { ... })`로 감싸고, `std::sync::mpsc`(또는
  `tokio::sync::oneshot`) 채널로 결과를 받아와 `async fn` 안에서 `await`한다.
- 그 클로저 안에서 `EmbeddedEditor::embed(...)` (및 `Plugin` 락 획득)을 **직접, 동기적으로**
  호출한다 — 커맨드 채널을 거치지 않는다.
- `WindowEvent::Resized` 핸들러는 이미 Tauri 메인 스레드에서 실행되는 콜백이므로
  ([lib.rs:765](../src-tauri/src/lib.rs#L765)), `pending_editor_rect`에 써두고 다른 스레드가
  나중에 집어가게 하지 말고 **그 자리에서 바로 `EmbeddedEditor::set_rect(...)`를 호출**하게
  바꾼다.
- `WindowEvent::CloseRequested` 핸들러가 지금 새 스레드(`ministudio-plugin-editor-close`)를
  또 스폰해서 `close_plugin_editor`를 부르는데
  ([lib.rs:783-791](../src-tauri/src/lib.rs#L783-L791)), 임베디드 에디터의 해제(Drop)도 같은
  스레드 요구사항을 가질 가능성이 높으므로 이것도 메인 스레드 경로로 옮기거나, 최소한
  `run_on_main_thread`를 거치도록 검토해야 함.

### 2) (별도, 훨씬 저비용) 플레이스홀더에 실패 상태 반영

`PluginEditorShell.tsx`가 임베딩 실패/타임아웃을 알 수 있는 이벤트가 지금 하나도 없음.
`engine_open_plugin_editor`가 에러를 낼 때 `PLUGIN_SHELL_STATE`류 이벤트에 실패 플래그를 얹어
해당 서브 창으로 보내고, 플레이스홀더 문구를 "연결 실패 - 다시 시도" 같은 상태로 바꿀 수 있게
하면, 위 1번 수정이 100% 끝나기 전이라도 최소한 "무한 로딩처럼 보이는" 체감 문제는 사라짐.
**근본 수정은 아니지만 사용자 체감 개선용으로 먼저 붙여도 됨.**

## 검증 순서 제안

1. 1번 수정(메인 스레드 이전) 적용 후 재현 — 플레이스홀더가 실제 플러그인 GUI로 바로 교체되는지
   확인.
2. 리사이즈(창 크기 조절 시 플러그인 GUI가 같이 리사이즈되는지)와 닫기(창 닫았다 다시 열기)도
   함께 확인 — 둘 다 같은 스레드 위반 패턴이 있었으므로 같이 고쳐야 회귀 없이 끝남.
3. VST3뿐 아니라 CLAP 플러그인으로도 동일 시나리오 확인 (`ClapControl`도 동일 구조).
4. 2번(실패 상태 UI)까지 붙이면, 혹시 남은 엣지케이스(특정 플러그인이 진짜로 파싱/초기화가
   오래 걸리는 경우)에도 사용자가 "무한 로딩"으로 오인하지 않음.
