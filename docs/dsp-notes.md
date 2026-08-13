# Rust DSP 노트

모든 이펙트는 `DspEffect`를 구현하고 사전 할당 `AudioBuffer`를 in-place 처리한다. `prepare`, `process`, `set_param`, `reset`, `tail_samples`, `latency_samples`가 실시간과 오프라인에서 같은 의미를 가진다.

## Parametric EQ

최대 6개 런타임 밴드를 전제로 RBJ Audio EQ Cookbook 계수를 사용한다. Bell, low/high shelf, HPF/LPF, notch를 Transposed Direct Form II로 처리하며 채널별 `z1/z2`가 독립이다. HPF/LPF 기울기는 12dB 섹션을 최대 네 개 캐스케이드한다. 응답 API는 20Hz–20kHz 로그 주파수에서 각 섹션의 복소 전달함수 크기를 합산한다.

## Compressor

검출 신호를 원폴 제곱 평균으로 RMS화하고 dB 로그 영역에서 threshold, ratio와 2차 soft knee를 적용한다. attack/release 계수는 `exp(-1 / (time × sampleRate))`이고 스테레오 채널은 큰 검출값으로 링크된다.

## Delay

2초 크기의 채널별 원형 버퍼에서 4점 Catmull–Rom 분수 지연을 읽는다. 지연 시간, feedback, mix는 샘플 단위로 평활화하고 피드백에는 원폴 댐핑 필터를 둔다. ping-pong 모드는 반대 채널의 지연 출력을 피드백한다.

## FDN Reverb

48kHz 기준 서로소 길이 8개를 샘플레이트에 비례해 할당하고 8×8 Hadamard 버터플라이로 피드백을 혼합한다. 라인별 RT60 이득은 `10^(-3 × length / (decay × sampleRate))`이다. 피드백 상태에 댐핑과 데놈럴 제거를 적용하며 `tail_samples()`는 decay에 대응한다.

## Waveshaper

Tanh, cubic soft clip, hard clip, arctan, foldback, asymmetric 곡선을 제공한다. 1×/2×/4× 내부 보간 샘플을 셰이핑하고 15-tap 대칭 half-band FIR로 저역통과한 뒤 다운샘플한다. 비대칭 곡선 뒤에는 약 5Hz DC blocker를 적용한다. 오버샘플 모드는 7샘플 FIR 군지연을 보고해 트랙 PDC에 포함된다.

## 검증 기준

Rust 테스트는 스무더 63% 시상수, EQ 유한 응답, 웨이브셰이퍼 지연 보고를 포함한다. 다음 네이티브 검증 단계에서는 임펄스 FFT 0.1dB EQ 오차, 컴프레서 정적 곡선, 분수 딜레이 피크, 리버브 RT60 ±15%, 오버샘플 alias 에너지 및 32–1024 프레임 블록 불변성을 자동화한다.
