# MIDI 노트 왜곡 조사 — "도-(괴상한 음)-미" 현상

## 해결 결과 (2026-08-15)

실제 설치된 Serum 1과 Serum 2를 MiniStudio VST3 어댑터로 직접 오프라인 렌더해 원인을 확정했다.
MIDI pitch, velocity, note-id, Note Off는 정상이었고 문제는 `vst3-host` 0.9의 버스 버퍼 프레임 수
판정과 MiniStudio의 고정 최대 버퍼 사용 방식 사이의 계약 불일치였다.

- MiniStudio는 재할당 방지를 위해 VST3 버스 채널을 2048샘플로 한 번 할당하고, 실제 콜백 크기는
  `BusAudioBuffers::block_size`에 256처럼 기록했다.
- `vst3-host` 0.9의 bus-aware 처리 경로는 이 필드보다 첫 채널 `Vec::len()`을 우선해 프레임 수를
  2048로 판정했다.
- 결과적으로 Serum의 내부 오실레이터는 콜백마다 2048샘플 전진했지만 MiniStudio는 앞의 256샘플만
  소비했다. 음높이별 블록 경계 위상에 따라 큰 불연속이 발생해 어떤 음은 정상이고 어떤 음은
  전화기/8비트 불협화음처럼 들렸다.

`prepare_vst_bus_block`이 매 콜백 채널 길이를 정확한 프레임 수로 truncate/resize한 뒤 0으로 지우도록
수정했다. 최초 2048샘플 용량은 유지되므로 오디오 스레드 재할당은 없다. 이 처리는 VST3 악기와
이펙트 양쪽에 공통 적용했다.

실장 플러그인 진단 결과:

| 플러그인 | MIDI 60 | MIDI 62 | MIDI 64 | Note Off 이후 RMS |
|---|---:|---:|---:|---:|
| Serum 1 | 261.638 Hz | 293.565 Hz | 329.739 Hz | 0.000000 |
| Serum 2 | 261.638 Hz | 293.565 Hz | 329.739 Hz | 0.000000 |

기대값은 각각 261.626, 293.665, 329.628 Hz다. 아래 내용은 해결 전 정적 분석 기록으로 보존한다.

> 분석 전용 문서. 코드 변경 없음. 실제 오디오 재생 테스트를 할 수 없는 환경에서 정적 분석으로만
> 진행했으므로, 아래 "남은 가설"은 반드시 재현 후 좁혀야 함.

## 증상

세럼(Serum)을 로드하고 "도레미"(순차적인 3개 음)를 연주하면, 첫 음과 마지막 음은 정상인데
**가운데 음만 이상하게(찢어지거나 음정이 어긋난 듯) 들린다.**

사용자 확인: 가상 피아노(CapsLock+QWERTY)와 실물 MIDI 컨트롤러 **둘 다** 이 증상이 나는 것 같고,
**피아노롤에 찍어서 재생해도 같은 현상이 날 것 같다**고 함 (미확인, 추정).
→ 만약 피아노롤 재생에서도 재현된다면, 라이브 입력 경로(가상피아노/하드웨어 MIDI)의 스레딩·타이밍
문제가 아니라 **세 경로가 공유하는 다운스트림(그래프/VST3 디스패치)** 쪽 문제일 가능성이 커짐.
가장 먼저 확정해야 할 사실이 이것.

## 관련 입력 경로 3가지

1. **가상 피아노 (JS)** — [VirtualPiano.tsx](../src/components/VirtualPiano.tsx)
   `startNote`/`stopNote` → `engine.midiNote(trackId, note.id, pitch, velocity, on)` →
   Rust `midi_note()` ([engine.rs:816-850](../src-tauri/crates/ministudio-audio/src/audio/engine.rs#L816-L850)) →
   `AudioCommand::LiveMidi` → `track.live_events`.
   `note.id`는 `useRef(3_000_000)`에서 시작하는 단조 증가 카운터라 **충돌 없음**.

2. **실물 MIDI 컨트롤러 (Rust, midir)** — `connect_midi_input()`
   ([engine.rs:975-1069](../src-tauri/crates/ministudio-audio/src/audio/engine.rs#L975-L1069)).
   `midir` 콜백 안에서 피치별 8-슬롯 LIFO 스택(`note_ids: [[i32;8];128]`, `note_counts: [usize;128]`)으로
   note-on/off를 짝지어 고유 `note_id`를 만들어 `LiveMidiMessage`를 `ArrayQueue`(용량 2048, 오버플로 시
   최신 이벤트 드롭)에 push.

3. **피아노롤(타임라인 스케줄) 재생** — [graph.rs:317-410](../src-tauri/crates/ministudio-audio/src/audio/graph.rs#L317-L410)
   (`AudioGraph::build`). 클립의 각 노트를 NoteOn/NoteOff 쌍으로 펼쳐 `midi_events: Vec<ScheduledNoteEvent>`에
   push한 뒤 `sort_by_key(|e| e.sample)`로 정렬. `note_id`는 `build()` 시작 시 `1`부터 시작하는
   그래프 전역 카운터.

세 경로 모두 `Graph::process()`에서 합류:
[graph.rs:630-658](../src-tauri/crates/ministudio-audio/src/audio/graph.rs#L630-L658) —
타임라인 이벤트 + 라이브 이벤트를 `track.event_buffer`에 모아 `sample_offset`으로 정렬한 뒤
`instrument.process(&track.event_buffer, &mut track.buffer, frames)` 호출.
Serum(VST3)의 경우 최종적으로 `Vst3Instrument::send_event`
([lib.rs:786-848](../src-tauri/crates/ministudio-plugin/src/lib.rs#L786-L848))를 거쳐
`vst3-host` 크레이트의 `note_on_at`/`note_off_at`으로 들어감.

## 배제한 가설 (코드로 확인 완료)

- **note_id 충돌**: 세 경로 모두 서로 다른 방식이지만 각자 유일성을 보장함 (위 설명 참조). 의심 낮음.
- **velocity 범위 오류**: `engine.rs`의 `midi_note()`와 하드웨어 콜백 모두
  `velocity.clamp(0.0, 1.0)`을 거치고, `Vst3Instrument::send_event`에서 다시
  `(velocity.clamp(0.0,1.0)*127.0).round() as u8`로 이중 방어됨.
- **블록 간 버퍼 누적**: `Graph::process` 최상단에서 매 블록 `track.buffer.clear(frames)`
  ([graph.rs:617](../src-tauri/crates/ministudio-audio/src/audio/graph.rs#L617)) 후 딱 한 번
  `instrument.process()` 호출. 누적/이중 처리 없음.
- **피아노롤 노트 배열이 시간순 정렬 안 되어 있어서 NoteOff/NoteOn 정렬이 뒤집히는 경우**:
  `addMidiNote`/`updateMidiNotes` 모두 매번
  `.sort((a,b) => a.startTicks - b.startTicks || a.pitch - b.pitch)`를 적용함
  ([projectStore.ts:931](../src/store/projectStore.ts#L931),
  [projectStore.ts:943](../src/store/projectStore.ts#L943)). 따라서 `clip.notes`는 항상 시작 시각
  순으로 정렬된 채로 Rust에 전달됨 → 인접 노트 경계에서 push 순서가 뒤집혀 NoteOn이 NoteOff보다
  먼저 정렬되는 시나리오는 배제됨.
- **`vst3-host` 크레이트의 note_on_at/note_off_at 자체 버그**: 크레이트 소스
  (`~/.cargo/registry/.../vst3-host-0.9.0/src/internal/plugin_impl.rs:3514-3573`)를 직접 확인함.
  `note_on`은 자체 단조 증가 `next_note_id`로 고유 ID를 만들고 `(id, channel, pitch)`를
  `active_notes`에 기록, `note_off`는 그 ID로 정확히 채널/피치를 복원해 이벤트를 만듦. 로직 자체는
  건전해 보임. 단, `note_on`의 오버플로 축출 로직(`active_notes.len() >= MAX_TRACKED_NOTES`일 때
  `swap_remove(0)`)은 `note_off`가 `swap_remove(i)`로 순서를 흐트러뜨린 뒤에는 "index 0 = 가장
  오래된 노트"라는 가정이 깨짐 — 다만 이건 다수 노트를 동시에 눌러 미해제 상태로 쌓아야 발동하는
  엣지케이스라 "도레미" 3음 재현과는 무관할 가능성이 높음.
- **`note_on_at`이 가끔 실패해서 Note-Expression 경로와 raw MIDI 폴백 경로가 노트마다 뒤섞이는 가설**:
  `note_on_at`은 `validate_note`/`validate_velocity`만 통과하면 되고, 우리 쪽에서 이미
  0-127 범위로 클램프해서 넘기므로 사실상 항상 성공(Ok)함. 폴백 경로(raw
  `MidiEvent::NoteOn`)는 거의 죽은 코드에 가까움 → 이 가설은 신뢰도 낮음으로 하향.

## 남은 유력 가설 (재현 후 검증 필요, 신뢰도순)

1. **[중] 세럼 패치 자체의 레가토/모노/포르타멘토 설정.**
   호스트가 노트를 정확히 순서대로 보내더라도, 패치가 모노+레가토/글라이드 모드면 겹치거나
   딱 붙어있는 노트 사이에서 피치가 이전 노트에서 "미끄러지는" 현상이 원래 그렇게 설계된 동작임.
   가운데 음만 이상하게 들리는 것과 정확히 일치하는 패턴 (첫 음은 무음에서 시작해 깨끗하고,
   마지막 음도 트리거만 깨끗하면 정상, 가운데 음만 "이전 노트에서 슬라이드해오는" 소리가 날 수 있음).
   → **DefaultSynth(내장 신스)로 같은 "도레미"를 쳐서 재현되는지가 가장 값싸고 결정적인 분기 테스트.**
   내장 신스에서 안 나면 세럼 패치/모노 모드 쪽으로 무게가 실림.

2. **[중] 그래프 리빌드와 라이브 노트 겹침으로 인한 note_id 유실.**
   `AudioGraph`는 프로젝트 변경마다 통째로 재생성되어 `retired` 채널로 교체됨
   ([engine.rs](../src-tauri/crates/ministudio-audio/src/audio/engine.rs) 내
   `Producer<Box<AudioGraph>>` 참조). `Vst3Instrument::active_notes`는 인스턴스 로컬 상태라,
   노트를 누른 채로 그래프가 재빌드되면(예: 피아노롤 편집, 트랙 파라미터 변경 등이 재생 중에
   일어난 경우) 새 그래프의 `Vst3Instrument`는 그 note_id의 매핑을 모름 → 이후 note-off가
   `active_notes.remove()`에서 못 찾고 raw MIDI 폴백으로 빠짐. 이게 "가운데 음만 이상함"과
   직접 인과관계가 있다고 확신은 못 하지만, 스턱노트/폴백 경로 전환 자체가 세럼에서 다른 보이스
   할당 경로를 타게 만들 수 있어 후보로 남겨둠. **재현 시 재생 중 프로젝트를 전혀 건드리지 않은
   상태로도 나는지 확인 필요.**

3. **[낮음, 하드웨어 경로 한정] 컨트롤러의 러닝 스테이터스/여분 MIDI 메시지.**
   `connect_midi_input` 콜백은 `message[0] & 0xf0`을 매 메시지 상태 바이트로 가정함
   ([engine.rs:1011](../src-tauri/crates/ministudio-audio/src/audio/engine.rs#L1011)). `midir`가
   OS 드라이버 단에서 완전한 메시지로 정규화해주는 게 보통이라 가능성은 낮지만, 컨트롤러가
   모드 휠/애프터터치 등 예상 못 한 메시지를 노트 사이에 흘려보내는 경우를 배제 못 함.
   **가상 피아노(순수 JS 경로)에서도 동일 증상이 100% 재현되는지 확인되면 이 가설은 완전히
   배제 가능.**

## 재현·진단 절차 제안 (수정 전에 먼저)

1. **입력 경로 고정**: 가상 피아노만으로 "도레미" 반복 재현 확인 (하드웨어 컨트롤러 변수 제거).
2. **플러그인 변수 제거**: 같은 시퀀스를 DefaultSynth(내장)로 재생 — 재현되면 호스트/그래프 문제,
   안 되면 VST3 특정 문제로 좁혀짐.
3. **타이밍 변수 제거**: 피아노롤에 노트 사이 확실한 간격(예: 반박자 쉼)을 두고 찍어서 재생 —
   간격을 주면 사라지는지 확인. 사라지면 레가토/경계-근접 타이밍 계열 가설(#1, #2)에 무게.
4. **로깅 계측**: `Vst3Instrument::send_event`
   ([lib.rs:786](../src-tauri/crates/ministudio-plugin/src/lib.rs#L786))에 임시로
   `note_id`, `pitch`, `offset`, `note_on_at` 성공 여부를 로그. `Graph::process`가
   `instrument.process()`를 호출하기 직전 `track.event_buffer`
   ([graph.rs:658](../src-tauri/crates/ministudio-audio/src/audio/graph.rs#L658) 바로 위)의
   내용도 함께 로그해서, 실제로 세 노트가 예상한 순서·간격대로 들어가는지 눈으로 확인.
5. **프로젝트 무편집 재생**: 재생 중 아무 것도 편집하지 않은 상태에서도 나는지 확인 (가설 #2 검증).

위 1~3번만 돌려도 가설 범위가 크게 좁혀질 것으로 보임. 결과를 바탕으로 코덱스가 수정에 들어가면 됨.
