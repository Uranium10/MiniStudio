# 간이 DAW — 1단계 구현 프롬프트

> 아래 전체를 에이전트에 그대로 전달하세요.

---

## 프로젝트 개요

Tauri v2 + React + TypeScript 기반의 데스크톱 간이 DAW를 만든다. 이번 단계에서는 **Web Audio API만으로 동작하는 오디오 엔진과 전체 UI**를 완성한다. 향후 단계에서 Rust 백엔드에 VST3 호스팅을 붙일 예정이므로, **그 교체가 UI 코드 수정 없이 가능하도록** 설계해야 한다.

## 기술 스택 (고정)

- Tauri v2 (`tauri` 2.11.x, `@tauri-apps/api` 2.11.x)
- React 18 + TypeScript + Vite
- 상태관리: Zustand
- 스타일: Tailwind CSS
- 오디오: Web Audio API (네이티브 라이브러리 사용 금지)
- 파일 I/O: `@tauri-apps/plugin-fs`, `@tauri-apps/plugin-dialog`
- 파형 렌더링: Canvas 2D 직접 구현 (wavesurfer.js 등 외부 라이브러리 금지)

## 최우선 설계 제약 — 엔진 추상화

이것이 이 프로젝트에서 **가장 중요한 요구사항**이다. 위반 시 전체를 다시 작성해야 한다.

React 컴포넌트와 Zustand 스토어는 **`AudioContext`, `AudioNode`, `AudioBuffer` 등 Web Audio 타입을 절대 직접 참조하지 않는다.** 모든 오디오 조작은 `IAudioEngine` 인터페이스를 통해서만 이루어진다.

```
src/engine/
  IAudioEngine.ts        # 인터페이스 정의 (구현 없음)
  types.ts               # 엔진 도메인 타입 (AudioNode 타입 노출 금지)
  webaudio/
    WebAudioEngine.ts    # 이번 단계 구현체
    ...
  index.ts               # createEngine() 팩토리 — 구현체 선택 지점
```

UI에서는 `const engine = useEngine()` 형태로만 접근한다. 나중에 `createEngine()`이 `RustEngine`을 반환하도록 한 줄만 바꿔도 UI가 그대로 동작해야 한다.

### IAudioEngine 인터페이스 (이대로 구현할 것)

```ts
export interface IAudioEngine {
  // 수명주기
  init(): Promise<void>;
  dispose(): Promise<void>;

  // 미디어
  loadAudioFile(path: string): Promise<AudioAssetInfo>;
  // AudioAssetInfo: { id, path, name, durationSec, sampleRate, numChannels, peaks: Float32Array }
  unloadAsset(assetId: string): Promise<void>;

  // 그래프 구성 (선언적 — 스토어 상태를 넘기면 엔진이 내부 그래프를 동기화)
  syncGraph(snapshot: GraphSnapshot): Promise<void>;

  // 트랜스포트
  play(fromSec?: number): Promise<void>;
  pause(): Promise<void>;
  stop(): Promise<void>;
  seek(sec: number): Promise<void>;
  getPlayheadSec(): number;
  onPlayhead(cb: (sec: number) => void): () => void;  // 구독 해제 함수 반환

  // 파라미터 (실시간, 그래프 재구성 없이)
  setTrackVolume(trackId: string, gainDb: number): void;
  setTrackPan(trackId: string, pan: number): void;   // -1..1
  setTrackMute(trackId: string, muted: boolean): void;
  setTrackSolo(trackId: string, solo: boolean): void;
  setSendLevel(sendId: string, gainDb: number): void;
  setEffectParam(effectId: string, paramId: string, value: number): void;

  // 미터링
  getTrackLevel(trackId: string): { peak: number; rms: number };
  getMasterLevel(): { peak: number; rms: number };

  // === 2단계 대비 (이번 단계에서는 미구현 스텁) ===
  scanPlugins(): Promise<PluginDescriptor[]>;
  renderOffline(req: OfflineRenderRequest): Promise<OfflineRenderResult>;

  // 능력 질의 — UI가 이걸 보고 기능을 노출/숨김
  capabilities(): EngineCapabilities;
  // { supportsExternalPlugins: boolean, supportsRealtimePluginInsert: boolean, ... }
}
```

`scanPlugins()`와 `renderOffline()`은 `WebAudioEngine`에서 각각 빈 배열 / `NotSupportedError` throw로 구현한다. `capabilities().supportsExternalPlugins`는 `false`를 반환하고, UI는 이 값이 `false`일 때 플러그인 관련 메뉴를 비활성 상태로(숨기지 말고 disabled + 툴팁) 표시한다.

## 데이터 모델

```ts
type ProjectState = {
  meta: { name: string; sampleRate: number; createdAt: string };
  transport: { bpm: number; playheadSec: number; isPlaying: boolean; loop: {enabled, startSec, endSec} };
  assets: Record<string, AudioAssetInfo>;
  tracks: Track[];
  buses: Bus[];        // 센드 목적지
  master: MasterStrip;
};

type Track = {
  id: string; name: string; color: string;
  clips: Clip[];
  volumeDb: number; pan: number; muted: boolean; solo: boolean;
  effects: EffectInstance[];   // 인서트 체인 (순서 있음)
  sends: Send[];
};

type Clip = {
  id: string; assetId: string;
  startSec: number;        // 타임라인상 위치
  offsetSec: number;       // 에셋 내부 시작 지점 (트리밍)
  durationSec: number;
  gainDb: number;
  fadeInSec: number; fadeOutSec: number;
};

type Send = { id: string; targetBusId: string; gainDb: number; preFader: boolean };
type Bus = { id: string; name: string; effects: EffectInstance[]; volumeDb: number };
type EffectInstance = { id: string; type: EffectType; bypassed: boolean; params: Record<string, number> };
```

## UI 레퍼런스 (중요)

이 앱의 UI는 두 DAW를 참조한다. 픽셀 단위 복제가 아니라 **인터랙션 모델과 레이아웃 구조**를 따른다.

- **상단 절반 (툴바 + 트랙 리스트 + 타임라인 + 좌측 인스펙터)** → **PreSonus Studio One 6** 방식
- **하단 절반 (믹서 + 이펙트 랙)** → **Ableton Live** 방식

### 전체 레이아웃

```
┌─────────────────────────────────────────────────────────────┐
│ 메뉴바                                                        │
├─────────────────────────────────────────────────────────────┤
│ 툴바: [도구 그룹] [스냅/퀀타이즈] [줌] [트랜스포트 옵션]        │  ← Studio One
├──────────┬──────────────────────────────────────────────────┤
│          │ 타임라인 룰러 / 마커 트랙                          │
│ 인스펙터  ├──────────────────────────────────────────────────┤
│ (선택된   │ 트랙 헤더 │ 아레인지 뷰 (클립 + 파형)              │  ← Studio One
│  트랙)    │  M S R ▮ │                                       │
│          │          │                                       │
├──────────┴──────────┴──────────────────────────────────────┤
│ ══ 드래그 가능한 스플리터 ══                                   │
├─────────────────────────────────────────────────────────────┤
│ 하단 패널 (탭: 믹서 / 디바이스)                                │  ← Ableton
│  믹서 탭: 세로 채널 스트립 가로 나열                            │
│  디바이스 탭: 선택 트랙의 이펙트가 가로로 나열된 랙              │
├─────────────────────────────────────────────────────────────┤
│ 트랜스포트 바: 시간표시 · 재생/정지/녹음 · BPM · 박자 · 루프    │
└─────────────────────────────────────────────────────────────┘
```

하단 패널은 스플리터로 높이 조절 가능하고, 완전히 접을 수 있다(단축키로 토글).

---

## 도구 시스템 (Studio One 방식) — 정밀 명세

이 프로젝트에서 **두 번째로 중요한 요구사항**이다. 대충 구현하면 안 된다.

### 도구 목록과 키 매핑

툴바에 아래 순서대로 아이콘 버튼을 배치하고, 숫자키에 매핑한다.

| 키 | 도구 ID | 이름 | 커서 | 동작 |
|---|---|---|---|---|
| 1 | `arrow` | 선택 | 화살표 | 클립 선택·이동·트리밍·페이드 핸들 |
| 2 | `range` | 범위 선택 | 십자 | 시간 구간 드래그 선택 (트랙 가로지르기 가능) |
| 3 | `split` | 스플릿 | 칼 | 클릭 지점에서 클립 분할 |
| 4 | `erase` | 지우개 | 지우개 | 클릭한 클립 삭제, 드래그로 연속 삭제 |
| 5 | `paint` | 그리기 | 펜 | 드래그로 빈 클립 생성 (오디오 트랙에서는 무음 클립) |
| 6 | `mute` | 뮤트 | X | 클릭한 클립 뮤트 토글, 드래그로 연속 토글 |
| 7 | `listen` | 오디션 | 스피커 | 마우스 누르는 동안 해당 지점부터 미리듣기 |

### 서브 도구 (스마트 툴) — 상태 머신

Studio One의 핵심 인터랙션이다. 정확히 아래 규칙대로 구현한다.

**개념**: `arrow` 도구는 "서브 도구"를 하나 물고 있을 수 있다. 서브 도구가 지정된 상태에서 <kbd>Ctrl</kbd>(macOS는 <kbd>Cmd</kbd>)을 누르고 있는 동안에만 그 도구로 일시 전환된다. 키를 떼면 즉시 `arrow`로 복귀한다.

**서브 도구 지정 방법**: <kbd>1</kbd>을 반복해서 누르면 서브 도구가 순환한다.

```
활성 도구가 arrow가 아닐 때 '1' 누름 → arrow로 전환 (서브 도구는 기존 값 유지)

활성 도구가 이미 arrow일 때 '1' 누름 → 서브 도구 순환:
  none → range → split → erase → paint → mute → none → ...
```

순환 시 툴바의 `arrow` 버튼에 현재 서브 도구 아이콘을 작게 오버레이로 표시하고, 화면 하단이나 툴바 근처에 짧은 토스트로 "서브 도구: 스플릿" 같이 알린다.

**Ctrl 홀드 동작**:
- 서브 도구가 `none`이면 <kbd>Ctrl</kbd>은 아무 효과 없음 (일반 다중선택 modifier로 동작)
- 서브 도구가 지정돼 있으면, <kbd>Ctrl</kbd> `keydown` 시 `effectiveTool = subTool`, `keyup` 시 `effectiveTool = arrow`
- **드래그 도중 Ctrl 상태가 바뀌어도 진행 중인 제스처의 도구는 바뀌지 않는다.** 도구는 `pointerdown` 시점에 확정(latch)되고, 그 제스처가 끝날 때까지 유지된다.
- 창이 포커스를 잃으면(`blur`) 모든 modifier 상태를 초기화한다. 그러지 않으면 Alt+Tab 후 Ctrl이 눌린 채로 남는 고전적 버그가 생긴다.

**상태 모델**:
```ts
type ToolState = {
  activeTool: ToolId;        // 툴바에서 선택된 도구
  subTool: ToolId | 'none';  // arrow의 서브 도구
  isModifierHeld: boolean;
  latchedTool: ToolId | null; // 진행 중인 제스처의 도구
};

// 파생값 — 컴포넌트는 이것만 본다
function getEffectiveTool(s: ToolState): ToolId {
  if (s.latchedTool) return s.latchedTool;
  if (s.activeTool === 'arrow' && s.subTool !== 'none' && s.isModifierHeld) return s.subTool;
  return s.activeTool;
}
```

`getEffectiveTool`은 순수 함수로 분리하고 **단위 테스트를 반드시 작성한다.** 위 규칙 전부(순환, latch, blur 초기화)를 케이스로 커버할 것.

### 도구별 히트 영역

`arrow` 도구일 때 클립 위에서 커서 위치에 따라 동작이 달라진다:
- 클립 좌/우 끝 8px 이내 → 트리밍 (커서: `ew-resize`)
- 클립 상단 모서리 삼각형 핸들 → 페이드 인/아웃 조절
- 그 외 본체 → 이동
- 빈 영역 드래그 → 사각형 다중 선택 (러버밴드)

커서 모양은 `effectiveTool`과 히트 영역에 따라 실시간으로 바뀌어야 한다.

---

## 구현 기능 (1단계 범위)

### 1. 트랙 & 타임라인
- 트랙 추가/삭제/이름변경/색상변경/순서변경(드래그)
- 타임라인에 오디오 클립 배치, 드래그로 이동, 좌우 엣지 드래그로 트리밍
- 클립 복제(Alt+드래그), 삭제, 스플릿(플레이헤드 위치에서 S키)
- 줌 인/아웃 (가로: 시간축, 세로: 트랙 높이), 스크롤 동기화
- 스냅 토글 (그리드 / 오프)

### 2. 파형 렌더링
- 파일 로드 시 **피크 데이터를 사전 계산**해 캐시한다. 원본 샘플을 매 프레임 순회하지 말 것.
- 피크 계산: 청크 단위 min/max 쌍을 여러 해상도(멀티 LOD)로 미리 만들어 두고, 현재 줌 레벨에 맞는 LOD를 선택해 그린다.
- Canvas 2D로 렌더. 클립 하나당 캔버스 하나가 아니라, **트랙 레인 전체를 하나의 캔버스**로 그려 DOM 노드 폭증을 막는다.
- 화면 밖 영역은 그리지 않는다 (뷰포트 컬링).
- 피크 계산은 `AudioContext.decodeAudioData` 이후 Web Worker에서 수행해 UI 블로킹을 막는다.

### 3. 믹서 (Ableton Live 방식)

하단 패널의 "믹서" 탭. 세로 채널 스트립을 가로로 나열하고, 좌우 스크롤한다.

각 채널 스트립의 세로 구성 (위 → 아래):
1. 트랙 색상 바 + 트랙명 (더블클릭으로 인라인 이름 편집)
2. **센드 노브 영역** — 버스 개수만큼 작은 노브가 세로로 쌓임. 각 노브 옆에 버스 이름 축약 표시. Ableton처럼 노브를 세로 드래그로 조절.
3. 팬 슬라이더 (가로 바, 중앙 스냅. 더블클릭 시 센터로 리셋)
4. 페이더 + 레벨 미터가 **나란히 붙어서** 배치 (Ableton 특유의 레이아웃). 페이더 옆에 현재 dB 값 숫자 표시.
5. 하단: M / S / (녹음 암) 버튼 한 줄

- 마스터 스트립은 우측 끝에 고정(스크롤해도 안 밀림), 시각적으로 구분되는 배경색
- 버스 스트립은 트랙과 마스터 사이에 배치, 살짝 다른 색조
- 페이더/노브 공통 인터랙션: 드래그로 조절, <kbd>Shift</kbd>+드래그로 미세 조절, 더블클릭으로 기본값 리셋, 값 텍스트 더블클릭으로 직접 입력
- 미터: peak + RMS 동시 표시, peak hold 1.5초, 클리핑 시 상단 인디케이터가 빨갛게 latch (클릭하면 해제)
- 채널 스트립 하나를 클릭하면 상단 아레인지의 해당 트랙이 선택되고 (양방향 동기화), 디바이스 탭이 그 트랙의 랙으로 전환된다

### 4. 내장 이펙터 (Web Audio 노드 조합으로 구현)
최소 아래 4종. 각각 인서트 체인에 순서대로 삽입 가능하고, 개별 바이패스 가능.
- **EQ** — 3밴드 (low shelf / peaking / high shelf), `BiquadFilterNode` 체인
- **Compressor** — `DynamicsCompressorNode` 래핑, threshold/ratio/attack/release/knee
- **Delay** — `DelayNode` + 피드백 게인 + wet/dry 믹스
- **Reverb** — `ConvolverNode` + 알고리즘으로 생성한 impulse response (외부 IR 파일 의존 금지, 노이즈 감쇠로 생성)

#### 디바이스 랙 UI (Ableton Live 방식)

하단 패널의 "디바이스" 탭. **선택된 트랙(또는 버스/마스터)의 이펙트 체인을 가로로 나열**한다.

- 각 디바이스는 **고정 높이, 가변 폭의 가로 패널**. 신호 흐름 순서대로 좌 → 우로 배치되고, 우측이 체인의 끝.
- 디바이스 패널 상단 타이틀 바: 전원 버튼(바이패스 토글) · 디바이스 이름 · 접기 버튼(세로로 접혀 얇은 막대가 됨)
- 패널 본체에는 파라미터 노브/슬라이더가 배치. 노브 아래 파라미터명, 위 또는 옆에 현재 값.
- **드래그로 순서 변경.** 드래그 중 삽입 지점에 세로 하이라이트 라인 표시.
- 랙 우측 빈 공간을 더블클릭하거나 `+` 버튼으로 디바이스 추가 (드롭다운 또는 브라우저 팝업)
- 디바이스 우클릭 → 삭제 / 복제 / 바이패스
- 랙이 화면 폭을 넘으면 가로 스크롤
- 비활성(바이패스) 디바이스는 전체적으로 채도를 낮춰 표시

EQ 디바이스에는 주파수 응답 곡선을 그리는 작은 캔버스를 포함시킨다(밴드 조작 시 실시간 반영). Compressor에는 게인 리덕션 미터를 포함시킨다.

각 이펙터는 공통 인터페이스를 갖는 클래스로 만들어, 나중에 VST3 인서트가 같은 자리에 들어올 수 있게 한다:
```ts
interface IEffectNode {
  readonly id: string;
  input: AudioNode; output: AudioNode;   // 엔진 내부에서만 접근
  setParam(paramId: string, value: number): void;
  setBypassed(b: boolean): void;
  dispose(): void;
}
```

### 5. 트랜스포트 & 재생
- 재생/정지/일시정지, 스페이스바 토글
- 플레이헤드: 타임라인 클릭으로 이동, 재생 중 부드럽게 진행 (`requestAnimationFrame` + `AudioContext.currentTime` 기준. `setInterval` 카운팅 금지)
- 루프 구간 설정 및 루프 재생
- **스케줄링**: 룩어헤드 스케줄러 패턴을 사용한다. 25ms마다 타이머가 돌며 향후 100ms 내에 시작해야 할 클립을 `AudioBufferSourceNode.start(when)`으로 예약한다. 클립 시작 시점에 즉석에서 `start()`를 호출하는 방식은 금지.
- 클립 페이드 인/아웃은 `GainNode`에 `linearRampToValueAtTime`으로 예약.

### 6. 파일 I/O
- 오디오 임포트: `@tauri-apps/plugin-dialog`의 `open()`으로 경로 선택 → `@tauri-apps/plugin-fs`의 `readFile()`로 바이트 읽기 → `decodeAudioData`.
  - 주의: `readFile`은 `Uint8Array`를 반환한다. `decodeAudioData`에 넘길 때 underlying `ArrayBuffer`를 올바르게 슬라이스할 것.
  - 지원: wav, mp3, flac, ogg, m4a (브라우저 디코더가 지원하는 범위)
- 드래그앤드롭 임포트도 지원 (Tauri의 파일 드롭 이벤트 사용)
- 프로젝트 저장/불러오기: `.json` 확장자로 `ProjectState` 직렬화. 오디오는 **절대경로 참조만 저장**하고 복사하지 않는다. 불러올 때 파일이 없으면 "누락된 파일" 목록을 띄우고 사용자가 재지정할 수 있게 한다.
- 마스터 출력을 wav로 익스포트: `OfflineAudioContext`로 전체 그래프를 렌더 후 wav 인코딩(직접 구현, 라이브러리 금지)

### 7. 단축키

**도구**: <kbd>1</kbd>~<kbd>7</kbd> (위 도구 시스템 명세 참조), <kbd>Ctrl</kbd> 홀드로 서브 도구

**트랜스포트**: <kbd>Space</kbd>(재생/정지), <kbd>Enter</kbd>(시작 지점으로), <kbd>L</kbd>(루프 토글)

**편집**: <kbd>Delete</kbd>(삭제), <kbd>Ctrl+Z</kbd>/<kbd>Ctrl+Shift+Z</kbd>(실행취소/재실행), <kbd>Ctrl+D</kbd>(복제), <kbd>Ctrl+A</kbd>(전체 선택)

**파일**: <kbd>Ctrl+S</kbd>(저장), <kbd>Ctrl+O</kbd>(열기), <kbd>Ctrl+Shift+E</kbd>(익스포트)

**뷰**: <kbd>+</kbd>/<kbd>-</kbd>(가로 줌), <kbd>Shift</kbd>+<kbd>+</kbd>/<kbd>-</kbd>(트랙 높이), <kbd>F</kbd>(전체 보기에 맞춤), <kbd>Tab</kbd>(하단 패널 접기/펼치기), <kbd>Shift+Tab</kbd>(믹서 ↔ 디바이스 탭 전환)

단축키는 중앙 레지스트리(`src/shortcuts/`)에 선언적으로 정의하고, 텍스트 입력 필드에 포커스가 있을 때는 발동하지 않게 한다. 나중에 사용자 커스텀 매핑을 붙일 수 있는 구조로.

실행취소는 Zustand 미들웨어로 `ProjectState` 스냅샷 스택을 관리. 오디오 에셋 바이너리는 스택에 넣지 말 것.

## 2단계(VST3) 밑준비 — 이번 단계에서 함께 작업할 것

실제 VST3 호스팅은 구현하지 않는다. 다만 **뼈대만 미리 세워둔다.**

### Rust 측 (`src-tauri/`)
아래 커맨드를 스텁으로 만들어 등록한다. 각각 지금은 빈 결과/에러를 반환하되, 시그니처와 직렬화 타입은 확정된 형태로 작성한다.

```rust
#[tauri::command]
async fn scan_vst3_plugins(paths: Vec<String>) -> Result<Vec<PluginDescriptor>, String>

#[tauri::command]
async fn render_offline_effect(req: OfflineRenderRequest) -> Result<OfflineRenderResult, String>
```

- `PluginDescriptor`: `{ uid, name, vendor, category, path, is_instrument, param_count }`
- `OfflineRenderRequest`: `{ plugin_uid, input_wav_path, output_wav_path, sample_rate, params: HashMap<String, f32> }`
- `OfflineRenderResult`: `{ output_wav_path, duration_sec, peak_db }`

`Cargo.toml`에 VST3 크레이트는 아직 추가하지 말 것. 주석으로 후보만 기록한다 (`vst3-host`, `rack`).

플랫폼별 기본 VST3 스캔 경로를 상수로 정의해 둔다:
- Windows: `C:\Program Files\Common Files\VST3`
- macOS: `/Library/Audio/Plug-Ins/VST3`, `~/Library/Audio/Plug-Ins/VST3`
- Linux: `~/.vst3`, `/usr/lib/vst3`

### TypeScript 측
- `IAudioEngine`의 `scanPlugins` / `renderOffline`이 위 Tauri 커맨드를 호출하는 얇은 래퍼를 `src/engine/native/` 에 만들어 둔다 (WebAudioEngine에서는 호출하지 않음).
- UI에 "플러그인" 패널을 만들되, `capabilities().supportsExternalPlugins === false`이면 "2단계에서 지원 예정" 안내와 함께 비활성 상태로 표시.
- 인서트 체인 UI는 내장 이펙터와 외부 플러그인을 **같은 슬롯 목록**으로 렌더하도록 설계 (`EffectInstance.type`이 `'builtin:eq'` | `'vst3:<uid>'` 형태를 취할 수 있게).

## 코드 품질 요구사항

- TypeScript strict 모드. `any` 금지.
- 오디오 스케줄링, 피크 계산, 그래프 동기화 등 순수 로직에는 단위 테스트를 작성한다 (Vitest).
- `src/engine/` 내부 파일은 React를 import 하지 않는다. 반대로 `src/components/`는 Web Audio 타입을 import 하지 않는다. 이 경계는 ESLint `no-restricted-imports` 규칙으로 강제한다.
- 각 모듈 상단에 역할을 한 줄로 주석 처리.

## 작업 순서

1. `npm create tauri-app@latest` (React + TS 템플릿), 플러그인(fs, dialog) 설치 및 권한 설정
2. `IAudioEngine` 인터페이스와 도메인 타입 정의 — **구현보다 먼저**
3. Zustand 스토어 + 데이터 모델
4. `WebAudioEngine` 골격 (init, loadAudioFile, syncGraph, 트랜스포트)
5. **도구 시스템** (`getEffectiveTool` 순수 함수 + 테스트 먼저, 그다음 툴바 UI)
6. 타임라인 UI + 파형 렌더링 + 도구별 클립 조작
7. 하단 패널 셸 (스플리터, 탭 전환)
8. 믹서 UI (Ableton 스타일) + 파라미터 연동
9. 내장 이펙터 4종 + 디바이스 랙 UI (Ableton 스타일)
10. 센드/버스 라우팅
11. 파일 I/O, 프로젝트 저장/불러오기, wav 익스포트
12. 2단계 스텁 (Rust 커맨드 + TS 래퍼 + 비활성 UI)
13. ESLint 경계 규칙, 테스트, README

각 단계마다 빌드가 통과하고 앱이 실행되는 상태를 유지할 것. 한 번에 전부 작성한 뒤 마지막에 디버깅하지 말 것.

## 산출물

- 동작하는 Tauri 앱 (`npm run tauri dev`로 실행 가능)
- `README.md`: 실행 방법, 아키텍처 개요, 엔진 교체 방법(2단계 진입 시 무엇을 어떻게 바꾸면 되는지)
- `docs/engine-contract.md`: `IAudioEngine` 계약 문서 — 각 메서드의 의미, 호출 순서 제약, 스레드/타이밍 가정

## 하지 말 것

- Web Audio 타입이 컴포넌트나 스토어로 새어 나오게 하는 것
- 실제 VST3 로딩 시도
- `localStorage` / `sessionStorage` 사용 (Tauri fs 플러그인으로 파일에 저장)
- 오디오 콜백(`ScriptProcessorNode`/`AudioWorklet`) 안에서 메모리 할당
- 파형 그리기에 외부 라이브러리 도입
- 도구 상태를 컴포넌트 로컬 state로 흩뿌리는 것 (반드시 단일 스토어 + `getEffectiveTool` 파생)
- 드래그 제스처 도중에 도구가 바뀌게 두는 것 (latch 규칙 위반)
- Studio One / Ableton의 아이콘·에셋을 그대로 가져다 쓰는 것 (레이아웃과 인터랙션만 참조하고, 아이콘은 Lucide 등 오픈 아이콘셋 사용)
