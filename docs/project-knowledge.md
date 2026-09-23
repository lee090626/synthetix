# 로컬 프로젝트 지식 시스템

이 도구는 프로젝트 파일, 문서, Tree-sitter 구조 정보, 로컬 Git 기록을 SQLite에 색인하고, 질문과 연결된 근거를 JSON 또는 텍스트로 반환한다. 답변 생성 모델은 포함하지 않는다. CLI 출력은 모델이 사용할 근거 묶음이며, 근거가 없는 질문은 결과가 없다고 알려야 한다.

## 아키텍처

```text
프로젝트 파일 ── 문서/설정 청크 ─┐
소스 코드 ── Tree-sitter 심볼/import ├─> SQLite: FTS5 + 관계 그래프
로컬 Git ── 커밋/변경 파일/브랜치 ┘             │
                                               ▼
질문 확장 → 전문 검색 → 관계 확장 → 현재 원본 재확인 → 근거 묶음
```

Python 표준 라이브러리의 `sqlite3`, `hashlib`, `subprocess`와 로컬 Git CLI를 쓴다. 코드 구조 추출에는 무료 Tree-sitter 언어 팩을 사용한다. 파서는 필요한 언어를 처음 사용할 때 내려받아 로컬 캐시에 두며, 이후 색인과 검색은 캐시된 파서로 실행된다. 로컬 LLM, 벡터 DB, 유료 API, 네트워크 검색은 사용하지 않는다.

## SQLite 스키마

| 테이블 | 역할 |
| --- | --- |
| `projects` | 색인한 프로젝트 루트와 마지막 파일/Git 갱신 상태 |
| `documents` | 파일 또는 커밋 원본, 종류, 언어, SHA-256, Git 메타데이터 |
| `chunks` / `chunk_fts` | 줄 범위가 있는 파일·심볼·문서·TODO·커밋 본문과 FTS5 전문 색인 |
| `nodes` | 파일, 디렉터리, 심볼, 모듈, 기능 ID, 문서 섹션, TODO, 커밋, 브랜치 |
| `edges` | 출처 청크가 연결된 방향 관계 |
| `changes` | 커밋별 변경 파일과 상태 |
| `branches` | 로컬 clone에 존재하는 브랜치 ref와 대상 커밋 |

FTS5는 경로, 종류, 심볼명, 본문을 검색한다. 임베딩은 만들지 않는다. SQLite 페이지 캐시를 작게 제한하고 파일을 하나씩 읽는다.

## 관계 모델

- 파일 시스템: `CONTAINS`
- AST 구조: 파일 `DECLARES` 심볼, 파일 `IMPORTS` 모듈
- 내부 import 경로가 관례적인 파일 경로와 정확히 맞을 때 모듈 `RESOLVES_TO` 파일
- 문서/코드에서 심볼명이 고유한 식별자로 나타나면 `MENTIONS_SYMBOL` (호출 여부까지 증명하지 않음)
- 문서: Markdown `LINKS_TO` 경로, 명시된 기능 ID `DOCUMENTED_IN`/`MENTIONED_IN`
- Git: 파일 `CHANGED_BY` 커밋, 브랜치 `POINTS_TO` 커밋, 커밋 `PARENT_COMMIT` 부모 커밋
- TODO: 파일 `HAS_TODO` TODO 항목

각 관계는 가능하면 그것을 만든 청크를 보유한다. 기능 ID나 문서의 문구만으로 코드가 구현됐다고 표시하지 않는다. 심볼명이 나온 것은 호출 그래프와 다르므로 `CALLS` 관계를 만들지 않는다. 이슈/PR은 원격 API로 읽지 않는다. clone에 실제 들어 있는 문서와 Git ref/커밋만 색인한다.

## 색인과 검색

1. Git 저장소는 `git ls-files`로 추적/비무시 파일을 찾는다. 비 Git 프로젝트는 일반적인 빌드·캐시 디렉터리를 건너뛴다.
2. 파일을 하나씩 읽어 SHA-256을 계산한다. 바뀐 파일만 파싱하고, 삭제된 파일의 청크와 관계는 제거한다. 기본 1 MiB 초과 파일과 바이너리는 건너뛴다. `.env`, 개인 키 파일 등은 건너뛰며 알려진 비밀 값 패턴은 색인/출력 전에 가린다.
3. Tree-sitter가 제공하는 구조 심볼과 import를 저장한다. Markdown은 제목 구간, 나머지는 줄 단위 청크로 나눈다. TODO/FIXME/XXX/HACK도 별도 색인한다.
4. 로컬 Git의 커밋 작성자/시각/제목/본문, 변경 파일, 브랜치 ref를 저장한다.
5. 질문의 원문과 코드 식별자 분해어를 FTS5에 검색한다. 상위 결과에서 파일, 심볼, 문서, 기능, 커밋 관계를 최대 두 단계 따라 추가한다.
6. 검색 전에 프로젝트 파일을 다시 순회하고 해시를 비교해 색인을 갱신한다. 반환 직전에도 인용 파일을 다시 읽고 해시가 일치하는지 확인한다. 인용에는 현재 파일 경로와 줄 범위 또는 전체 커밋 SHA가 들어간다.

검색 점수는 후보를 정렬하는 데만 쓰며 사실 판정이 아니다. `missing` 또는 `stale` 근거는 사실로 내보내지 않는다. 문서, 코드 구조, Git 이력은 서로 다른 근거 종류로 유지한다.

## 사용

Python 3.11+와 Git이 필요하다. SQLite 빌드에 FTS5가 활성화돼야 한다.

```sh
python3 -m pip install -r tools/project-knowledge/requirements.txt
python3 tools/project-knowledge/knowledge.py index /path/to/project
python3 tools/project-knowledge/knowledge.py search "이 기능은 어디에 구현되어 있는가?" --project /path/to/project
python3 tools/project-knowledge/knowledge.py status
```

Tree-sitter grammar를 받지 못해 AST가 빠진 코드 파일은 `status`와 검색 경고에 표시된다. 이후 `index /path/to/project --force-parse`로 다시 시도할 수 있다.

기본 DB는 `$XDG_DATA_HOME/project-knowledge/index.sqlite` 또는 `~/.local/share/project-knowledge/index.sqlite`이다. 여러 프로젝트는 같은 DB에 색인할 수 있다. 모델에 전달하기 쉬운 JSON이 기본 출력이며 `--format text`는 터미널용이다.

## 알려진 범위

- 로컬 검색/색인은 무료다. Tree-sitter grammar는 언어별 최초 사용 시 캐시를 준비해야 하므로 그때 인터넷이 필요할 수 있다. 캐시가 준비된 뒤에는 오프라인으로 쓸 수 있다.
- FTS5는 형태소 검색이 아니다. 한국어 조사가 붙거나 표현이 크게 다르면 검색어를 바꿔 추가 질의해야 한다.
- GitHub 이슈/PR 본문은 clone에 저장되지 않으므로 자동으로 포함하지 않는다.
- 검색은 구현 상태를 증명하지 않는다. 최종 답변은 반환 근거의 종류와 줄/커밋을 따로 인용해야 한다.
