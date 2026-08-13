# MiniDAW

Studio One의 아레인지 워크플로와 Ableton Live의 믹서·디바이스 랙 구조를 참고한 Tauri 2 기반 데스크톱 DAW입니다. 2단계부터 모든 재생·믹싱·DSP는 Rust 네이티브 엔진이 소유하며 React/Zustand UI에는 네이티브 오디오 타입이 노출되지 않습니다.

## 실행과 검증

Node.js 20 이상, Rust stable, 운영체제별 Tauri v2 사전 요구사항이 필요합니다. Windows 기본 빌드는 Visual Studio Build Tools의 Desktop development with C++ 및 Windows SDK가 필요합니다.

```bash
npm install
npm run tauri dev
```

Windows에서는 `run.bat`을 사용하면 Cargo/MSVC 환경과 외부 빌드 캐시,
실시간 DSP 최적화 개발 프로필이 자동으로 설정됩니다.

```bash
npm run build
npm test
npm run lint
npm run bindings
cd src-tauri && cargo test --no-default-features
cd src-tauri && cargo build --bin minidaw
```

브라우저에서 `npm run dev`로 UI를 볼 수 있지만 네이티브 오디오 IPC는 Tauri 프로세스에서만 동작합니다. Rust 단위 테스트와 바인딩 생성은 GUI 런타임을 링크하지 않는 `--no-default-features` 구성을 사용하고, 실제 앱은 기본 `desktop` 기능으로 빌드합니다. Windows에서는 MSVC 링커가 포함된 개발자 환경에서 실행해야 합니다.

## 네이티브 오디오 구조

- `src/engine/IAudioEngine.ts`: UI가 의존하는 유일한 오디오 계약
- `src/engine/rust/bindings.ts`: Rust에서 자동 생성되는 typed invoke 바인딩
- `src/engine/rust/RustEngine.ts`: 재생 30Hz/정지 4Hz 상태 캐시 어댑터
- `src-tauri/src/audio/engine.rs`: 제어 평면, CPAL 스트림, 락프리 큐와 트리플 버퍼
- `src-tauri/src/audio/graph.rs`: 샘플 단위 클립, 트랙/센드/버스/마스터, PDC와 공용 렌더 그래프
- `src-tauri/src/audio/asset.rs`: Symphonia 디코딩, Rubato 리샘플링, 멀티 LOD 피크
- `src-tauri/src/audio/dsp/`: 직접 구현한 EQ, 컴프레서, 딜레이, FDN 리버브, 오버샘플 웨이브셰이퍼

트랙뿐 아니라 리턴 버스와 마스터도 동일한 디바이스 랙에서 FX를 추가·정렬·바이패스·삭제할 수 있습니다. 믹서에서 대상 스트립을 클릭하면 해당 체인으로 전환됩니다.

EQ 곡선은 `engine_eq_response`가 돌려주는 네이티브 필터 실측 응답을 그립니다. 브라우저 미리보기처럼 IPC가 없는 환경에서만 근사 곡선으로 대체됩니다.

버스 뮤트와 마스터 뮤트·딤은 UI 측 게인 오프셋입니다. 네이티브 그래프에는 스트립 게인 하나만 존재하므로 `effectiveBusGainDb`/`effectiveMasterGainDb`로 계산한 실효 게인이 전달됩니다.

## 아직 없는 기능

메트로놈과 카운트인, 오디오 입력 녹음, 오토메이션 레인, 리턴 버스 레벨 미터, CPU 부하 계측은 모두 Rust 엔진 변경이 필요해 아직 없습니다. 하단 트랜스포트의 상태 표시는 엔진이 실제로 측정하는 출력 레이턴시·xrun·PDC만 보여줍니다.

Rust 커맨드 시그니처나 IPC 구조체를 바꾸면 반드시 `npm run bindings`를 실행하고 생성된 `src/engine/rust/bindings.ts`를 함께 커밋합니다. 파형 피크는 base64가 아니라 little-endian raw f32 IPC로 전달됩니다. 전체 계약은 [IPC 문서](docs/ipc-contract.md)에 정리되어 있습니다.

## ASIO와 라이선스

기본 빌드는 운영체제 기본 백엔드(WASAPI/CoreAudio/ALSA)만 사용하며 ASIO를 포함하지 않습니다.

```bash
npm run tauri build -- --features asio
```

`asio` 피처는 `cpal/asio`와 Steinberg ASIO SDK를 포함합니다. **ASIO 피처를 활성화한 빌드는 GPLv3 적용 대상입니다.** 해당 조건을 검토하지 않은 배포에는 사용하지 마세요. 피처가 꺼지면 ASIO 백엔드는 장치 목록에도 나타나지 않습니다.

## 3단계 진입점

VST3 플러그인 UI 슬롯과 `scanPlugins()`/`renderOffline()` 계약은 유지되어 있지만 현재 capability는 `false`입니다. 3단계에서는 Rust 그래프의 `DspEffect` 슬롯을 VST3 프로세서 어댑터로 확장하며 UI나 프로젝트 스토어를 교체하지 않습니다. VST3 SDK는 2025년 10월 이후 MIT 라이선스 기준을 전제로 합니다.

상세 내용은 [오디오 아키텍처](docs/audio-architecture.md), [DSP 노트](docs/dsp-notes.md), [엔진 계약](docs/engine-contract.md), [성능 노트](docs/performance.md)를 참고하세요.
