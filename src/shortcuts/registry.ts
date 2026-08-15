// Declarative keyboard shortcut registry for display and dispatch.
export type ShortcutId =
  | 'transport.toggle' | 'transport.home' | 'transport.loop' | 'transport.loopSelection'
  | 'edit.delete' | 'edit.split' | 'edit.undo' | 'edit.redo' | 'edit.duplicate' | 'edit.selectAll'
  | 'edit.copy' | 'edit.cut' | 'edit.paste' | 'edit.mute' | 'edit.quantize'
  | 'file.new' | 'file.save' | 'file.open' | 'file.export'
  | 'view.zoomIn' | 'view.zoomOut' | 'view.trackIn' | 'view.trackOut' | 'view.fit' | 'view.panel' | 'view.tab'
  | 'view.follow' | 'view.shortcuts' | 'view.pianoRoll' | 'view.bottomPanel' | 'view.inspector' | 'view.browser'
  | 'tool.pick' | 'tool.temporary' | 'midi.transpose' | 'midi.duplicate' | 'midi.octave' | 'timeline.wheelZoom' | 'timeline.wheelScroll'

export type ShortcutDefinition = { id: ShortcutId; label: string; keys: string }

export const shortcutGroups: ReadonlyArray<{ title: string; items: readonly ShortcutDefinition[] }> = [
  {
    title: '트랜스포트',
    items: [
      { id: 'transport.toggle', label: '재생 / 일시정지', keys: 'Space' },
      { id: 'transport.home', label: '처음으로', keys: 'Enter / B' },
      { id: 'transport.loop', label: '루프 켜기 / 끄기', keys: 'L' },
      { id: 'transport.loopSelection', label: '선택 영역을 루프로', keys: 'Shift+L' },
    ],
  },
  {
    title: '편집',
    items: [
      { id: 'edit.undo', label: '실행취소', keys: 'Ctrl+Z' },
      { id: 'edit.redo', label: '다시 실행', keys: 'Ctrl+Shift+Z' },
      { id: 'edit.copy', label: '복사', keys: 'Ctrl+C' },
      { id: 'edit.cut', label: '잘라내기', keys: 'Ctrl+X' },
      { id: 'edit.paste', label: '플레이헤드에 붙여넣기', keys: 'Ctrl+V' },
      { id: 'edit.duplicate', label: '제자리 복제', keys: 'Ctrl+D' },
      { id: 'edit.selectAll', label: '전체 선택', keys: 'Ctrl+A' },
      { id: 'edit.split', label: '플레이헤드에서 분할', keys: 'S' },
      { id: 'edit.mute', label: '선택 클립 뮤트', keys: 'M' },
      { id: 'edit.delete', label: '선택 삭제', keys: 'Delete' },
      { id: 'edit.quantize', label: '선택 노트/아이템 퀀타이즈', keys: 'Q' },
    ],
  },
  {
    title: '파일',
    items: [
      { id: 'file.new', label: '새 프로젝트', keys: 'Ctrl+N' },
      { id: 'file.open', label: '프로젝트 열기', keys: 'Ctrl+O' },
      { id: 'file.save', label: '프로젝트 저장', keys: 'Ctrl+S' },
      { id: 'file.export', label: '내보내기', keys: 'Ctrl+Shift+E' },
    ],
  },
  {
    title: '보기',
    items: [
      { id: 'view.zoomIn', label: '가로 확대', keys: 'E / +' },
      { id: 'view.zoomOut', label: '가로 축소', keys: 'W / -' },
      { id: 'view.trackIn', label: '전체 트랙 높이 늘리기', keys: 'Shift+E' },
      { id: 'view.trackOut', label: '전체 트랙 높이 줄이기', keys: 'Shift+W' },
      { id: 'view.fit', label: '전체 보기', keys: 'F' },
      { id: 'view.follow', label: '오토 스크롤', keys: 'Shift+F' },
      { id: 'view.panel', label: '하단 패널 접기 / 펴기', keys: 'Tab' },
      { id: 'view.tab', label: '믹서 / 디바이스 전환', keys: 'Shift+Tab' },
      { id: 'view.shortcuts', label: '단축키 도움말', keys: 'F1' },
      { id: 'view.pianoRoll', label: '피아노롤 열기 / 닫기', keys: 'F2' },
      { id: 'view.bottomPanel', label: '하단 인터페이스 열기 / 닫기', keys: 'F3' },
      { id: 'view.inspector', label: '인스펙터 열기 / 닫기', keys: 'F4' },
      { id: 'view.browser', label: '미디어 브라우저 열기 / 닫기', keys: 'F5' },
      { id: 'timeline.wheelZoom', label: '커서 기준 가로 줌', keys: 'Ctrl+휠' },
      { id: 'timeline.wheelScroll', label: '가로 스크롤', keys: 'Shift+휠' },
    ],
  },
  {
    title: '도구 · MIDI',
    items: [
      { id: 'tool.pick', label: '도구 선택 (선택/범위/스플릿/지우개/그리기/뮤트/오디션)', keys: '1 – 7' },
      { id: 'tool.temporary', label: '서브 도구 임시 전환', keys: 'Ctrl 누른 채' },
      { id: 'midi.transpose', label: '선택 노트 이동', keys: '↑ / ↓ · Ctrl+↑ / ↓ = 옥타브' },
      { id: 'midi.duplicate', label: '포커스된 노트/클립을 다음 구간에 복제', keys: 'D' },
      { id: 'midi.octave', label: '가상 피아노 토글 · 한 옥타브 연주', keys: 'CapsLock · Q2W3ER5T6Y7U' },
    ],
  },
]

export const shortcutRegistry: readonly ShortcutDefinition[] = shortcutGroups.flatMap((group) => group.items)
