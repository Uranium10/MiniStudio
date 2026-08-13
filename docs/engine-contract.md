# MiniDAW 오디오 엔진 계약

## 경계

`IAudioEngine`은 UI/프로젝트 모델과 Rust 오디오 런타임 사이의 유일한 경계다. React 컴포넌트와 Zustand 스토어는 CPAL, 네이티브 핸들 또는 Web Audio 타입을 보관하지 않는다. `src/engine/index.ts`는 `RustEngine`을 생성하고 어댑터는 Tauri 커맨드만 호출한다.

## 수명주기와 그래프

1. `init()`이 호스트와 기본 출력 장치를 열고 CPAL 스트림을 시작한다.
2. 구조 변경은 `syncGraph(snapshot)`으로 완성된 새 그래프를 제어 스레드에서 만든 뒤 오디오 콜백에 포인터 교체 명령으로 전달한다.
3. 볼륨·팬·뮤트·솔로·센드·이펙트 파라미터는 개별 락프리 명령으로 전달한다.
4. `dispose()`가 스트림과 제어 리소스를 정리한다.

교체된 이전 그래프는 반환 링버퍼를 통해 제어 측으로 돌아온 뒤 해제된다. 오디오 콜백은 그래프를 할당하거나 drop하지 않는다.

## 에셋과 트랜스포트

`loadAudioFile(path)`는 Symphonia로 f32 planar 데이터를 디코딩하고 필요하면 프로젝트 레이트로 리샘플링한다. 메타데이터는 자동 생성 typed command로, 멀티 LOD 피크는 별도 `ipc::Response`의 little-endian raw f32로 전달한 뒤 `Float32Array`로 복원된다.

내부 시간은 `u64` 샘플 위치다. `play`, `pause`, `stop`, `seek`만 IPC를 사용한다. `getPlayheadSec`, `getTrackLevel`, `getMasterLevel`, `getStreamStatus`는 재생 30Hz/정지 4Hz 폴링으로 채운 TypeScript 캐시를 동기 반환한다. 폴링 미터는 그래프 순서 배열과 revision으로 매핑한다.

## 장치 설정

`listAudioBackends`, `listOutputDevices`, `getAudioSettings`, `setAudioSettings`가 호스트·장치·샘플레이트·버퍼를 관리한다. 설정 변경은 스트림을 다시 열며 실패하면 기본 장치 폴백을 시도한다. `StreamStatus`는 계산 지연, xrun, 실행 상태, 오류와 PDC 샘플 수를 포함한다.

## 익스포트와 3단계

`exportProject()`는 실시간과 같은 `AudioGraph`/`DspEffect` 구현을 워커 커맨드에서 블록 단위로 실행해 WAV를 기록한다. `renderOffline()`은 외부 플러그인 1개를 위한 3단계 스텁이므로 별도다. `supportsExternalPlugins`는 3단계까지 `false`다.
