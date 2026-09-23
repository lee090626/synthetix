#!/usr/bin/env python3
"""Local, evidence-first project index backed by SQLite FTS5 and Git."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import sqlite3
import subprocess
import sys
import time
from pathlib import Path, PurePosixPath

MAX_FILE_BYTES = 1_048_576
MAX_CHUNK_LINES = 80
MAX_CHUNK_CHARS = 6_000
MAX_RESULTS = 50
MAX_GRAPH_NODES = 100
MAX_GRAPH_EVIDENCE = 12
FEATURE_ID = re.compile(r"\b(?:FUNC|FEATURE|REQ|STORY|TASK)[-_ ]?\d+(?:[-_]\d+)*\b", re.I)
TODO_LINE = re.compile(r"\b(TODO|FIXME|XXX|HACK|BUG)\b[: ]?(.*)", re.I)
IDENTIFIER = re.compile(r"[A-Za-z_][A-Za-z0-9_]{2,}")
COMMON_IDENTIFIERS = {
    "self", "this", "super", "main", "new", "default", "test", "run", "get", "set",
    "from", "into", "type", "class", "func", "function", "module", "value", "data",
    "name", "path", "result", "error", "item", "args", "config", "option", "options",
}
MARKDOWN_HEADING = re.compile(r"^ {0,3}(#{1,6})\s+(.+?)\s*#*\s*$")
MARKDOWN_LINK = re.compile(r"!?\[[^\]]*\]\(([^)]+)\)")
SENSITIVE_NAMES = {".env", ".envrc", ".npmrc", ".pypirc", ".netrc", "id_rsa", "id_ed25519", "credentials.json"}
SENSITIVE_EXTENSIONS = {".pem", ".key", ".p12", ".pfx", ".secret", ".token"}
SECRET_ASSIGNMENT = re.compile(
    r"(?i)(\b(?:[A-Za-z0-9]*(?:password|passwd|secret|token)|api[_-]?key|access[_-]?key|client[_-]?secret|private[_-]?key)\b\s*[:=]\s*)(\"[^\"]*\"|'[^']*'|[^\s,;#}]+)"
)
AUTHORIZATION_LINE = re.compile(r"(?im)(\b(?:authorization|proxy-authorization)\b\s*[:=]\s*)[^\r\n]+")
SECRET_TOKEN = re.compile(r"(?i)\b(?:AKIA[0-9A-Z]{16}|ghp_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{30,}|sk-[A-Za-z0-9_-]{24,})\b")
PRIVATE_KEY_BLOCK = re.compile(r"-----BEGIN [^-]*PRIVATE KEY-----.*?-----END [^-]*PRIVATE KEY-----", re.S)
SKIP_DIRS = {
    ".git", ".hg", ".svn", ".idea", ".vscode-test", "node_modules", "target",
    "dist", "build", "coverage", "vendor", "__pycache__", ".pytest_cache",
    ".mypy_cache", ".ruff_cache", ".venv", "venv", "env", ".next", ".nuxt",
}
CODE_LANGUAGES = {
    ".rs": "rust", ".py": "python", ".pyi": "python", ".js": "javascript",
    ".jsx": "javascript", ".mjs": "javascript", ".cjs": "javascript",
    ".ts": "typescript", ".tsx": "tsx", ".go": "go", ".java": "java",
    ".c": "c", ".h": "c", ".cc": "cpp", ".cpp": "cpp", ".cxx": "cpp",
    ".hpp": "cpp", ".hh": "cpp", ".cs": "c_sharp", ".rb": "ruby",
    ".php": "php", ".sh": "bash", ".bash": "bash", ".swift": "swift",
    ".kt": "kotlin", ".kts": "kotlin", ".scala": "scala", ".sql": "sql",
    ".lua": "lua", ".dart": "dart", ".ex": "elixir", ".exs": "elixir",
    ".hs": "haskell", ".zig": "zig", ".ax": "axiom",
}
DOC_EXTENSIONS = {".md", ".markdown", ".rst", ".adoc", ".txt"}
CONFIG_EXTENSIONS = {
    ".toml", ".json", ".yaml", ".yml", ".ini", ".cfg", ".conf", ".properties",
    ".xml", ".env.example", ".lock",
}
KOREAN_PARTICLES = (
    "으로부터", "에서부터", "에게서", "한테서", "으로써", "으로서", "에게", "한테",
    "에서는", "으로", "에서", "부터", "까지", "처럼", "보다", "에게는", "한테는",
    "은", "는", "이", "가", "을", "를", "의", "에", "로", "와", "과", "도", "만",
)
QUERY_EXPANSIONS = {
    "어디": ("파일", "경로", "함수", "모듈", "symbol", "function"),
    "구현": ("implementation", "symbol", "function", "class"),
    "왜": ("이유", "commit", "rationale", "decision", "history"),
    "이유": ("commit", "rationale", "decision", "because"),
    "변경": ("changed", "commit", "history", "diff"),
    "최근": ("recent", "commit", "change"),
    "사용처": ("reference", "import", "caller", "symbol"),
    "명세": ("spec", "requirement", "contract", "api"),
    "불일치": ("spec", "implementation", "documentation", "code"),
    "where": ("file", "path", "function", "symbol"),
    "why": ("commit", "rationale", "decision", "history"),
    "changed": ("commit", "history", "diff"),
    "callers": ("reference", "import", "symbol"),
    "specification": ("requirement", "contract", "api"),
}


SCHEMA = """
CREATE TABLE IF NOT EXISTS projects (
    id INTEGER PRIMARY KEY,
    root TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    indexed_at INTEGER NOT NULL DEFAULT 0,
    max_file_bytes INTEGER NOT NULL,
    git_fingerprint TEXT,
    git_head TEXT,
    git_ref TEXT
);
CREATE TABLE IF NOT EXISTS documents (
    id INTEGER PRIMARY KEY,
    project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    source_kind TEXT NOT NULL,
    path TEXT NOT NULL,
    kind TEXT NOT NULL,
    language TEXT,
    sha256 TEXT,
    size INTEGER,
    mtime_ns INTEGER,
    parse_status TEXT NOT NULL DEFAULT 'not-applicable',
    commit_sha TEXT,
    author TEXT,
    authored_at TEXT,
    subject TEXT,
    UNIQUE(project_id, source_kind, path)
);
CREATE TABLE IF NOT EXISTS chunks (
    id INTEGER PRIMARY KEY,
    document_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL,
    kind TEXT NOT NULL,
    symbol TEXT,
    start_line INTEGER,
    end_line INTEGER,
    body TEXT NOT NULL,
    UNIQUE(document_id, ordinal)
);
CREATE VIRTUAL TABLE IF NOT EXISTS chunk_fts USING fts5(
    path, kind, symbol, body,
    tokenize='unicode61 remove_diacritics 2'
);
CREATE TABLE IF NOT EXISTS nodes (
    id INTEGER PRIMARY KEY,
    project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    key TEXT NOT NULL,
    label TEXT NOT NULL,
    document_id INTEGER REFERENCES documents(id) ON DELETE CASCADE,
    chunk_id INTEGER REFERENCES chunks(id) ON DELETE CASCADE,
    UNIQUE(project_id, kind, key)
);
CREATE TABLE IF NOT EXISTS edges (
    source_id INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    target_id INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    relation TEXT NOT NULL,
    evidence_chunk_id INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY(source_id, target_id, relation, evidence_chunk_id)
);
CREATE TABLE IF NOT EXISTS commits (
    project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    sha TEXT NOT NULL,
    parents TEXT NOT NULL,
    author TEXT NOT NULL,
    authored_at TEXT NOT NULL,
    subject TEXT NOT NULL,
    body TEXT NOT NULL,
    PRIMARY KEY(project_id, sha)
);
CREATE TABLE IF NOT EXISTS changes (
    project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    commit_sha TEXT NOT NULL,
    path TEXT NOT NULL,
    status TEXT NOT NULL,
    chunk_id INTEGER REFERENCES chunks(id) ON DELETE CASCADE,
    PRIMARY KEY(project_id, commit_sha, path, status)
);
CREATE TABLE IF NOT EXISTS branches (
    project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    ref_name TEXT NOT NULL,
    kind TEXT NOT NULL,
    commit_sha TEXT NOT NULL,
    PRIMARY KEY(project_id, ref_name)
);
CREATE INDEX IF NOT EXISTS documents_project_kind ON documents(project_id, source_kind, kind);
CREATE INDEX IF NOT EXISTS nodes_project_kind_label ON nodes(project_id, kind, label);
CREATE INDEX IF NOT EXISTS edges_source ON edges(source_id);
CREATE INDEX IF NOT EXISTS edges_target ON edges(target_id);
CREATE INDEX IF NOT EXISTS changes_path ON changes(project_id, path);
CREATE INDEX IF NOT EXISTS commits_date ON commits(project_id, authored_at);
"""


def default_db_path() -> Path:
    data_home = os.environ.get("XDG_DATA_HOME")
    base = Path(data_home).expanduser() if data_home else Path.home() / ".local" / "share"
    return base / "project-knowledge" / "index.sqlite"


def connect_db(path: Path) -> sqlite3.Connection:
    path = path.expanduser().resolve()
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    existed = path.exists()
    conn = sqlite3.connect(path, timeout=30)
    if not existed:
        os.chmod(path, 0o600)
    conn.row_factory = sqlite3.Row
    conn.execute("PRAGMA foreign_keys=ON")
    conn.execute("PRAGMA cache_size=-8192")
    conn.execute("PRAGMA temp_store=FILE")
    conn.execute("PRAGMA journal_mode=WAL")
    conn.execute("PRAGMA synchronous=NORMAL")
    try:
        conn.executescript(SCHEMA)
    except sqlite3.OperationalError as exc:
        if "fts5" in str(exc).lower():
            raise RuntimeError("이 Python의 SQLite에 FTS5가 없습니다. FTS5가 포함된 Python/SQLite를 사용하세요.") from exc
        raise
    return conn


def need_treesitter():
    try:
        from tree_sitter_language_pack import ProcessConfig, process
    except ImportError as exc:
        raise RuntimeError(
            "Tree-sitter가 필요합니다. `python3 -m pip install -r tools/project-knowledge/requirements.txt`를 실행하세요."
        ) from exc
    return ProcessConfig, process


def text_chunks(text: str, kind: str, symbol: str | None = None, start_line: int = 1):
    lines = text.splitlines()
    if not lines:
        return
    offset = 0
    while offset < len(lines):
        end = min(offset + MAX_CHUNK_LINES, len(lines))
        while end > offset + 1 and sum(len(line) + 1 for line in lines[offset:end]) > MAX_CHUNK_CHARS:
            end -= 1
        body = "\n".join(lines[offset:end])
        yield kind, symbol, start_line + offset, start_line + end - 1, body
        if end == len(lines):
            break
        offset = max(offset + 1, end - 8)


def markdown_chunks(text: str):
    lines = text.splitlines()
    headings = []
    fence = None
    for index, line in enumerate(lines):
        marker = re.match(r"^ {0,3}(`{3,}|~{3,})", line)
        if marker:
            char = marker.group(1)[0]
            if fence == char:
                fence = None
            elif fence is None:
                fence = char
            continue
        if fence:
            continue
        match = MARKDOWN_HEADING.match(line)
        if match:
            headings.append((index, len(match.group(1)), match.group(2).strip()))
    if not headings:
        yield from text_chunks(text, "document")
        return
    if headings[0][0] > 0:
        yield from text_chunks("\n".join(lines[:headings[0][0]]), "document", "Introduction", 1)
    ends = [len(lines)] * len(headings)
    stack = []
    for i, (start, level, _) in enumerate(headings):
        while stack and headings[stack[-1]][1] >= level:
            ends[stack.pop()] = start
        stack.append(i)
    for i, (start, _, title) in enumerate(headings):
        end = ends[i]
        yield from text_chunks("\n".join(lines[start:end]), "section", title, start + 1)


def source_kind(path: str) -> tuple[str, str | None]:
    name = PurePosixPath(path).name.lower()
    suffix = PurePosixPath(path).suffix.lower()
    if name in {"dockerfile", "makefile", "justfile"}:
        return "code", {"dockerfile": "dockerfile", "makefile": "make", "justfile": "make"}[name]
    if suffix in CODE_LANGUAGES:
        return "code", CODE_LANGUAGES[suffix]
    if suffix in DOC_EXTENSIONS:
        return "documentation", None
    if suffix in CONFIG_EXTENSIONS or name.startswith(".env.example") or name.endswith(".toml"):
        return "configuration", None
    return "text", None


def sensitive_path(rel: str) -> bool:
    path = PurePosixPath(rel)
    name = path.name.lower()
    if any(part.lower() in {".aws", ".ssh", "secrets", "credentials"} for part in path.parts):
        return True
    if name in SENSITIVE_NAMES or path.suffix.lower() in SENSITIVE_EXTENSIONS:
        return True
    if name.startswith(".env") and not any(tag in name for tag in ("example", "sample", "template")):
        return True
    return False


def redact_secrets(text: str) -> str:
    text = PRIVATE_KEY_BLOCK.sub(
        lambda match: "[REDACTED PRIVATE KEY]" + "\n" * match.group(0).count("\n"), text,
    )
    text = AUTHORIZATION_LINE.sub(r"\1[REDACTED]", text)
    text = SECRET_ASSIGNMENT.sub(r"\1[REDACTED]", text)
    return SECRET_TOKEN.sub("[REDACTED TOKEN]", text)


def iter_nul(stream):
    buffer = b""
    while True:
        part = stream.read(65_536)
        if not part:
            break
        buffer += part
        while b"\0" in buffer:
            token, buffer = buffer.split(b"\0", 1)
            yield token
    if buffer:
        yield buffer


def git_root(root: Path) -> Path | None:
    try:
        result = subprocess.run(
            ["git", "-C", str(root), "rev-parse", "--show-toplevel"],
            check=True, capture_output=True, text=True,
        )
        return Path(result.stdout.strip()).resolve()
    except (OSError, subprocess.CalledProcessError):
        return None


def iter_files(root: Path, git_dir: Path | None):
    if git_dir:
        process = subprocess.Popen(
            ["git", "-C", str(root), "ls-files", "--cached", "--others", "--exclude-standard", "-z"],
            stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        )
        assert process.stdout is not None
        for raw in iter_nul(process.stdout):
            rel = raw.decode("utf-8", "replace").replace(os.sep, "/")
            if rel:
                yield rel
        if process.wait() != 0:
            error = process.stderr.read().decode("utf-8", "replace") if process.stderr else ""
            raise RuntimeError(f"git ls-files 실패: {error.strip()}")
        return
    def raise_walk_error(error):
        raise error

    for current, dirs, names in os.walk(root, followlinks=False, onerror=raise_walk_error):
        dirs[:] = [name for name in dirs if name not in SKIP_DIRS]
        for name in names:
            path = Path(current, name)
            try:
                rel = path.relative_to(root).as_posix()
            except ValueError:
                continue
            yield rel


def ensure_project(conn: sqlite3.Connection, root: Path, max_file_bytes: int) -> int:
    root = root.resolve()
    conn.execute(
        "INSERT INTO projects(root,name,max_file_bytes) VALUES(?,?,?) "
        "ON CONFLICT(root) DO UPDATE SET name=excluded.name,max_file_bytes=excluded.max_file_bytes",
        (str(root), root.name, max_file_bytes),
    )
    return conn.execute("SELECT id FROM projects WHERE root=?", (str(root),)).fetchone()[0]


def insert_chunk(conn: sqlite3.Connection, document_id: int, path: str, ordinal: int,
                 kind: str, symbol: str | None, start: int | None, end: int | None, body: str) -> int:
    cur = conn.execute(
        "INSERT INTO chunks(document_id,ordinal,kind,symbol,start_line,end_line,body) VALUES(?,?,?,?,?,?,?)",
        (document_id, ordinal, kind, symbol, start, end, body),
    )
    chunk_id = cur.lastrowid
    conn.execute(
        "INSERT INTO chunk_fts(rowid,path,kind,symbol,body) VALUES(?,?,?,?,?)",
        (chunk_id, path, kind, symbol or "", body),
    )
    return chunk_id


def clear_document_chunks(conn: sqlite3.Connection, document_id: int):
    conn.execute("DELETE FROM chunk_fts WHERE rowid IN (SELECT id FROM chunks WHERE document_id=?)", (document_id,))
    conn.execute("DELETE FROM chunks WHERE document_id=?", (document_id,))


def extract_file_chunks(text: str, kind: str, language: str | None, parser_api):
    if kind == "documentation":
        return list(markdown_chunks(text)), "not-applicable", []
    chunks = list(text_chunks(text, "source" if kind == "code" else kind))
    parse_status = "not-applicable"
    imports = []
    if kind != "code" or not language:
        return chunks, parse_status, imports
    ProcessConfig, process = parser_api
    try:
        result = process(text, ProcessConfig(
            language=language,
            structure=True,
            imports=True,
            max_source_bytes=max(MAX_FILE_BYTES, len(text.encode("utf-8"))),
            parse_timeout_ms=2_000,
        ))
        parse_status = "parsed"
        lines = text.splitlines()

        def add_items(items, parents=()):
            for item in items:
                span = item.span
                start_line = int(span.start_line) + 1
                end_line = int(span.end_line) + (0 if int(span.end_column) == 0 else 1)
                end_line = max(start_line, end_line)
                selected = lines[start_line - 1:end_line]
                symbol = getattr(item, "name", None) or str(getattr(item, "kind", "symbol"))
                text_value = "\n".join(selected)
                for part_kind, part_symbol, part_start, part_end, body in text_chunks(
                    text_value, "symbol", symbol, start_line,
                ):
                    chunks.append((part_kind, part_symbol, part_start, part_end, body))
                add_items(getattr(item, "children", ()), parents + (symbol,))

        add_items(getattr(result, "structure", ()))
        for imported in getattr(result, "imports", ()):
            source = getattr(imported, "source", None)
            if not source:
                continue
            span = getattr(imported, "span", None)
            start = int(span.start_line) + 1 if span else None
            end = int(span.end_line) + (0 if int(span.end_column) == 0 else 1) if span else None
            imports.append(str(source))
            selected = lines[(start - 1 if start else 0):(end if end else len(lines))]
            body = "\n".join(selected) or f"import {source}"
            chunks.append(("import", str(source), start, end, body))
    except Exception as exc:  # Keep searchable source even when a grammar is absent or cannot parse it.
        error_name = type(exc).__name__
        parse_status = "unsupported" if "LanguageNotFound" in error_name else "error"
    return chunks, parse_status, imports


def add_file(conn: sqlite3.Connection, project_id: int, root: Path, rel: str,
             max_file_bytes: int, parser_api, force_parse: bool):
    if sensitive_path(rel):
        return "sensitive"
    absolute = root / Path(rel)
    try:
        if not absolute.resolve().is_relative_to(root.resolve()):
            return "outside-root"
        stat = absolute.stat()
        if not absolute.is_file() or stat.st_size > max_file_bytes:
            return "too-large"
        raw = absolute.read_bytes()
    except FileNotFoundError:
        return "missing"
    except (OSError, ValueError):
        return "unreadable"
    if b"\0" in raw[:8192]:
        return "binary"
    digest = hashlib.sha256(raw).hexdigest()
    text = raw.decode("utf-8-sig", "replace")
    if text.count("\ufffd") > max(8, len(text) // 100):
        return "binary"
    kind, language = source_kind(rel)
    previous = conn.execute(
        "SELECT id,sha256,parse_status FROM documents WHERE project_id=? AND source_kind='file' AND path=?",
        (project_id, rel),
    ).fetchone()
    if previous and previous["sha256"] == digest and not (force_parse and kind == "code"):
        conn.execute("UPDATE documents SET size=?,mtime_ns=? WHERE id=?", (stat.st_size, stat.st_mtime_ns, previous["id"]))
        return "unchanged"
    chunks, parse_status, _ = extract_file_chunks(redact_secrets(text), kind, language, parser_api)
    for line_no, line in enumerate(text.splitlines(), 1):
        match = TODO_LINE.search(line)
        if match:
            chunks.append(("todo", match.group(1).upper(), line_no, line_no, redact_secrets(line.strip())))
    if previous:
        document_id = previous["id"]
        clear_document_chunks(conn, document_id)
        conn.execute(
            "UPDATE documents SET kind=?,language=?,sha256=?,size=?,mtime_ns=?,parse_status=? WHERE id=?",
            (kind, language, digest, stat.st_size, stat.st_mtime_ns, parse_status, document_id),
        )
        outcome = "updated"
    else:
        cur = conn.execute(
            "INSERT INTO documents(project_id,source_kind,path,kind,language,sha256,size,mtime_ns,parse_status) "
            "VALUES(?,'file',?,?,?,?,?,?,?)",
            (project_id, rel, kind, language, digest, stat.st_size, stat.st_mtime_ns, parse_status),
        )
        document_id = cur.lastrowid
        outcome = "added"
    for ordinal, (chunk_kind, symbol, start, end, body) in enumerate(chunks):
        insert_chunk(conn, document_id, rel, ordinal, chunk_kind, symbol, start, end, body)
    return outcome


def git_facts(root: Path):
    repo = git_root(root)
    if not repo:
        return None
    try:
        head = subprocess.run(["git", "-C", str(root), "rev-parse", "HEAD"], check=True,
                              capture_output=True, text=True).stdout.strip()
        head_ref_result = subprocess.run(["git", "-C", str(root), "symbolic-ref", "--quiet", "--short", "HEAD"],
                                         capture_output=True, text=True)
        head_ref = head_ref_result.stdout.strip() if head_ref_result.returncode == 0 else "DETACHED"
        refs_raw = subprocess.run(
            ["git", "-C", str(root), "for-each-ref", "--format=%(refname)%00%(objectname)%00"],
            check=True, capture_output=True,
        ).stdout
    except (OSError, subprocess.CalledProcessError):
        return None
    refs = []
    tokens = refs_raw.split(b"\0")
    for i in range(0, len(tokens) - 1, 2):
        ref = tokens[i].decode("utf-8", "replace").strip()
        sha = tokens[i + 1].decode("ascii", "replace").strip()
        if ref.startswith(("refs/heads/", "refs/remotes/", "refs/pull/", "refs/merge-requests/")) and sha:
            kind = "local" if ref.startswith("refs/heads/") else "remote" if ref.startswith("refs/remotes/") else "pull"
            refs.append((ref, kind, sha))
    fingerprint = hashlib.sha256((head + "\n" + head_ref + "\n" + "\n".join(f"{r}\t{s}" for r, _, s in refs)).encode()).hexdigest()
    return repo, head, head_ref, refs, fingerprint


def git_log_events(root: Path, path_prefix: str | None = None):
    command = ["git", "-C", str(root), "log", "--all", "--no-renames", "-z",
               "--format=%H%x00%P%x00%an%x00%aI%x00%s%x00%b%x00", "--name-status"]
    if path_prefix:
        command.extend(("--", ":(literal)" + path_prefix))
    process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    assert process.stdout is not None
    tokens = iter_nul(process.stdout)
    pending = None
    while True:
        try:
            token = pending if pending is not None else next(tokens)
            pending = None
        except StopIteration:
            break
        sha = token.lstrip(b"\r\n").decode("ascii", "replace")
        if not re.fullmatch(r"[0-9a-fA-F]{40,64}", sha):
            continue
        fields = []
        try:
            for _ in range(5):
                fields.append(next(tokens).decode("utf-8", "replace"))
        except StopIteration:
            break
        parents, author, authored_at, subject, body = fields
        yield "commit", (sha, parents, author, authored_at, subject, body.strip())
        while True:
            try:
                token = next(tokens)
            except StopIteration:
                pending = None
                break
            status = token.lstrip(b"\r\n").decode("utf-8", "replace")
            if re.fullmatch(r"[0-9a-fA-F]{40,64}", status):
                pending = token
                break
            if not re.fullmatch(r"[A-Z][0-9]*", status):
                continue
            try:
                path = next(tokens).decode("utf-8", "replace")
            except StopIteration:
                pending = None
                break
            yield "change", (sha, status, path)
        if pending is None:
            # The stream may have ended after a commit's last path.
            try:
                token = next(tokens)
            except StopIteration:
                break
            pending = token
    if process.wait() != 0:
        error = process.stderr.read().decode("utf-8", "replace") if process.stderr else ""
        raise RuntimeError(f"git log 실패: {error.strip()}")


def add_git_document(conn: sqlite3.Connection, project_id: int, data):
    sha, parents, author, authored_at, subject, body = data
    subject = redact_secrets(subject)
    body = redact_secrets(body)
    path = f"git:{sha}"
    combined = f"{subject}\n{body}".strip()
    digest = hashlib.sha256((parents + author + authored_at + combined).encode()).hexdigest()
    row = conn.execute(
        "SELECT id,sha256 FROM documents WHERE project_id=? AND source_kind='git' AND path=?",
        (project_id, path),
    ).fetchone()
    if row:
        document_id = row["id"]
        if row["sha256"] != digest:
            clear_document_chunks(conn, document_id)
            conn.execute(
                "UPDATE documents SET sha256=?,kind='commit',author=?,authored_at=?,subject=?,commit_sha=? WHERE id=?",
                (digest, author, authored_at, subject, sha, document_id),
            )
            insert_chunk(conn, document_id, path, 0, "commit", sha[:12], None, None,
                         f"{subject}\nAuthor: {author}\nDate: {authored_at}\n{body}".strip())
    else:
        cur = conn.execute(
            "INSERT INTO documents(project_id,source_kind,path,kind,sha256,commit_sha,author,authored_at,subject) "
            "VALUES(?,'git',?,'commit',?,?,?,?,?)",
            (project_id, path, digest, sha, author, authored_at, subject),
        )
        document_id = cur.lastrowid
        insert_chunk(conn, document_id, path, 0, "commit", sha[:12], None, None,
                     f"{subject}\nAuthor: {author}\nDate: {authored_at}\n{body}".strip())
    conn.execute(
        "INSERT INTO commits(project_id,sha,parents,author,authored_at,subject,body) VALUES(?,?,?,?,?,?,?) "
        "ON CONFLICT(project_id,sha) DO UPDATE SET parents=excluded.parents,author=excluded.author,"
        "authored_at=excluded.authored_at,subject=excluded.subject,body=excluded.body",
        (project_id, sha, parents, author, authored_at, subject, body),
    )


def sync_git(conn: sqlite3.Connection, project_id: int, root: Path, facts):
    previous = conn.execute("SELECT git_fingerprint FROM projects WHERE id=?", (project_id,)).fetchone()[0]
    if facts is None:
        if previous is not None:
            git_docs = conn.execute("SELECT id FROM documents WHERE project_id=? AND source_kind='git'", (project_id,)).fetchall()
            for row in git_docs:
                clear_document_chunks(conn, row["id"])
            conn.execute("DELETE FROM documents WHERE project_id=? AND source_kind='git'", (project_id,))
            conn.execute("DELETE FROM commits WHERE project_id=?", (project_id,))
            conn.execute("DELETE FROM changes WHERE project_id=?", (project_id,))
            conn.execute("DELETE FROM branches WHERE project_id=?", (project_id,))
            conn.execute("UPDATE projects SET git_fingerprint=NULL,git_head=NULL,git_ref=NULL WHERE id=?", (project_id,))
            return True
        return False
    repo, head, head_ref, refs, fingerprint = facts
    if fingerprint == previous:
        return False
    conn.execute("CREATE TEMP TABLE IF NOT EXISTS seen_commits(sha TEXT PRIMARY KEY) WITHOUT ROWID")
    conn.execute("DELETE FROM seen_commits")
    conn.execute("DELETE FROM changes WHERE project_id=?", (project_id,))
    stale_change_chunks = conn.execute(
        "SELECT c.id FROM chunks c JOIN documents d ON d.id=c.document_id "
        "WHERE d.project_id=? AND d.source_kind='git' AND c.kind IN ('change','branch')", (project_id,),
    ).fetchall()
    for chunk in stale_change_chunks:
        conn.execute("DELETE FROM chunk_fts WHERE rowid=?", (chunk[0],))
        conn.execute("DELETE FROM chunks WHERE id=?", (chunk[0],))
    prefix = Path(root).resolve().relative_to(repo).as_posix()
    if prefix == ".":
        prefix = None
    for event, data in git_log_events(repo, prefix):
        if event == "commit":
            add_git_document(conn, project_id, data)
            conn.execute("INSERT OR IGNORE INTO seen_commits(sha) VALUES(?)", (data[0],))
        else:
            sha, status, path = data
            if prefix:
                if not path.startswith(prefix.rstrip("/") + "/"):
                    continue
                path = path[len(prefix.rstrip("/")) + 1:]
            doc = conn.execute(
                "SELECT id FROM documents WHERE project_id=? AND source_kind='git' AND path=?",
                (project_id, f"git:{sha}"),
            ).fetchone()
            if not doc:
                continue
            ordinal = conn.execute("SELECT COALESCE(MAX(ordinal),-1)+1 FROM chunks WHERE document_id=?", (doc[0],)).fetchone()[0]
            body = f"{status} {path}"
            chunk_id = insert_chunk(conn, doc[0], f"git:{sha}", ordinal, "change", path, None, None, body)
            conn.execute(
                "INSERT OR REPLACE INTO changes(project_id,commit_sha,path,status,chunk_id) VALUES(?,?,?,?,?)",
                (project_id, sha, path, status, chunk_id),
            )
    stale_commit_chunks = conn.execute(
        "SELECT c.id FROM chunks c JOIN documents d ON d.id=c.document_id "
        "WHERE d.project_id=? AND d.source_kind='git' AND d.commit_sha NOT IN "
        "(SELECT sha FROM seen_commits)", (project_id,),
    ).fetchall()
    for chunk in stale_commit_chunks:
        conn.execute("DELETE FROM chunk_fts WHERE rowid=?", (chunk[0],))
    conn.execute(
        "DELETE FROM documents WHERE project_id=? AND source_kind='git' AND commit_sha NOT IN "
        "(SELECT sha FROM seen_commits)", (project_id,),
    )
    conn.execute("DELETE FROM commits WHERE project_id=? AND sha NOT IN (SELECT sha FROM seen_commits)", (project_id,))
    conn.execute("DELETE FROM branches WHERE project_id=?", (project_id,))
    conn.executemany(
        "INSERT INTO branches(project_id,ref_name,kind,commit_sha) VALUES(?,?,?,?)",
        [(project_id, ref, kind, sha) for ref, kind, sha in refs],
    )
    for ref, kind, sha in refs:
        doc = conn.execute(
            "SELECT id FROM documents WHERE project_id=? AND source_kind='git' AND path=?",
            (project_id, f"git:{sha}"),
        ).fetchone()
        if doc:
            ordinal = conn.execute("SELECT COALESCE(MAX(ordinal),-1)+1 FROM chunks WHERE document_id=?",
                                   (doc[0],)).fetchone()[0]
            insert_chunk(conn, doc[0], f"git:{sha}", ordinal, "branch", ref, None, None,
                         f"{kind} branch {ref} points to {sha}")
    conn.execute(
        "UPDATE projects SET git_fingerprint=?,git_head=?,git_ref=? WHERE id=?",
        (fingerprint, head, head_ref, project_id),
    )
    return True


def node(conn: sqlite3.Connection, project_id: int, kind: str, key: str, label: str,
         document_id: int | None = None, chunk_id: int | None = None) -> int:
    conn.execute(
        "INSERT INTO nodes(project_id,kind,key,label,document_id,chunk_id) VALUES(?,?,?,?,?,?) "
        "ON CONFLICT(project_id,kind,key) DO UPDATE SET "
        "document_id=COALESCE(excluded.document_id,nodes.document_id),"
        "chunk_id=COALESCE(excluded.chunk_id,nodes.chunk_id),"
        "label=CASE WHEN nodes.label=nodes.key THEN excluded.label ELSE nodes.label END",
        (project_id, kind, key, label, document_id, chunk_id),
    )
    return conn.execute("SELECT id FROM nodes WHERE project_id=? AND kind=? AND key=?", (project_id, kind, key)).fetchone()[0]


def edge(conn: sqlite3.Connection, source: int, target: int, relation: str, evidence: int | None = None):
    conn.execute("INSERT OR IGNORE INTO edges(source_id,target_id,relation,evidence_chunk_id) VALUES(?,?,?,?)",
                 (source, target, relation, evidence or 0))


def rebuild_graph(conn: sqlite3.Connection, project_id: int):
    conn.execute("DELETE FROM nodes WHERE project_id=?", (project_id,))
    file_rows = conn.execute(
        "SELECT id,path,kind,language FROM documents WHERE project_id=? AND source_kind='file' ORDER BY path",
        (project_id,),
    )
    for doc in file_rows:
        file_node = node(conn, project_id, "file", doc["path"], doc["path"], doc["id"])
        parts = PurePosixPath(doc["path"]).parts
        parent_id = None
        for i in range(1, len(parts)):
            directory = "/".join(parts[:i])
            directory_id = node(conn, project_id, "directory", directory, directory)
            if parent_id:
                edge(conn, parent_id, directory_id, "CONTAINS")
            parent_id = directory_id
        if parent_id:
            edge(conn, parent_id, file_node, "CONTAINS")
        chunks = conn.execute(
            "SELECT id,ordinal,kind,symbol,start_line,body FROM chunks WHERE document_id=? ORDER BY ordinal",
            (doc["id"],),
        )
        for chunk in chunks:
            section_id = None
            chunk_node = node(conn, project_id, "chunk", f"{doc['path']}#chunk:{chunk['ordinal']}",
                              chunk["symbol"] or f"{chunk['kind']} in {doc['path']}", doc["id"], chunk["id"])
            if chunk["kind"] == "symbol" and chunk["symbol"]:
                symbol_id = node(conn, project_id, "symbol",
                                 f"{doc['path']}#{chunk['symbol']}:{chunk['start_line']}",
                                 chunk["symbol"], doc["id"], chunk["id"])
                edge(conn, file_node, symbol_id, "DECLARES", chunk["id"])
            elif chunk["kind"] == "import" and chunk["symbol"]:
                module_id = node(conn, project_id, "module", f"{doc['language'] or ''}:{chunk['symbol']}", chunk["symbol"])
                edge(conn, file_node, module_id, "IMPORTS", chunk["id"])
            elif chunk["kind"] == "section" and chunk["symbol"]:
                section_id = node(conn, project_id, "section",
                                  f"{doc['path']}#{chunk['symbol']}:{chunk['start_line']}",
                                  chunk["symbol"], doc["id"], chunk["id"])
                edge(conn, file_node, section_id, "CONTAINS_SECTION", chunk["id"])
            elif chunk["kind"] == "todo":
                todo_id = node(conn, project_id, "todo", f"{doc['path']}:{chunk['start_line']}",
                               chunk["body"].strip(), doc["id"], chunk["id"])
                edge(conn, file_node, todo_id, "HAS_TODO", chunk["id"])
            for feature_id in set(FEATURE_ID.findall(chunk["body"])):
                feature_id = feature_id.upper()
                feature_node = node(conn, project_id, "feature", feature_id, feature_id)
                relation = "DOCUMENTED_IN" if chunk["kind"] == "section" else "MENTIONED_IN"
                edge(conn, feature_node, chunk_node, relation, chunk["id"])
            if chunk["kind"] == "section":
                for target in MARKDOWN_LINK.findall(chunk["body"]):
                    target = target.split("#", 1)[0].strip().strip("<>")
                    if not target or "://" in target or target.startswith(("/", "#")):
                        continue
                    target_path = str(PurePosixPath(doc["path"]).parent.joinpath(target))
                    target_path = os.path.normpath(target_path).replace(os.sep, "/")
                    if target_path == ".." or target_path.startswith("../"):
                        continue
                    target_doc = conn.execute(
                        "SELECT id FROM documents WHERE project_id=? AND source_kind='file' AND path=?",
                        (project_id, target_path),
                    ).fetchone()
                    if target_doc:
                        target_node = node(conn, project_id, "file", target_path, target_path, target_doc[0])
                        edge(conn, section_id or chunk_node, target_node, "LINKS_TO", chunk["id"])
    # Resolve documentation references after every source file has contributed symbols.
    for doc in conn.execute(
        "SELECT id,path FROM documents WHERE project_id=? AND source_kind='file' AND kind IN ('documentation','text')",
        (project_id,),
    ):
        for chunk in conn.execute(
            "SELECT id,ordinal,kind,body FROM chunks WHERE document_id=? AND kind IN ('section','document')",
            (doc["id"],),
        ):
            chunk_node = node(conn, project_id, "chunk", f"{doc['path']}#chunk:{chunk['ordinal']}",
                              f"{chunk['kind']} in {doc['path']}", doc["id"], chunk["id"])
            for symbol_name in set(IDENTIFIER.findall(chunk["body"])):
                matches = conn.execute(
                    "SELECT id FROM nodes WHERE project_id=? AND kind='symbol' AND label=? LIMIT 2",
                    (project_id, symbol_name),
                ).fetchall()
                if len(matches) == 1:
                    edge(conn, chunk_node, matches[0][0], "MENTIONS_SYMBOL", chunk["id"])

    for doc in conn.execute(
        "SELECT id,path FROM documents WHERE project_id=? AND source_kind='file' AND kind='code'",
        (project_id,),
    ):
        for chunk in conn.execute(
            "SELECT id,ordinal,body FROM chunks WHERE document_id=? AND kind='source'",
            (doc["id"],),
        ):
            names = sorted(set(IDENTIFIER.findall(chunk["body"])) - COMMON_IDENTIFIERS)[:200]
            if not names:
                continue
            placeholders = ",".join("?" for _ in names)
            matches = conn.execute(
                f"SELECT id,label,document_id FROM nodes WHERE project_id=? AND kind='symbol' AND document_id<>? "
                f"AND label IN ({placeholders})",
                (project_id, doc["id"], *names),
            ).fetchall()
            by_name = {}
            for match in matches:
                by_name.setdefault(match["label"], []).append(match["id"])
            chunk_node = node(conn, project_id, "chunk", f"{doc['path']}#chunk:{chunk['ordinal']}",
                              f"source in {doc['path']}", doc["id"], chunk["id"])
            for name, symbol_ids in by_name.items():
                if len(symbol_ids) == 1:
                    edge(conn, chunk_node, symbol_ids[0], "MENTIONS_SYMBOL", chunk["id"])

    for imported in conn.execute(
        "SELECT c.id AS chunk_id,c.symbol,d.path,d.language FROM chunks c "
        "JOIN documents d ON d.id=c.document_id WHERE d.project_id=? AND d.source_kind='file' AND c.kind='import'",
        (project_id,),
    ):
        module_key = f"{imported['language'] or ''}:{imported['symbol']}"
        module_row = conn.execute("SELECT id FROM nodes WHERE project_id=? AND kind='module' AND key=?",
                                  (project_id, module_key)).fetchone()
        if not module_row:
            continue
        for target_path in module_file_candidates(imported["path"], imported["language"], imported["symbol"]):
            target = conn.execute(
                "SELECT id FROM nodes WHERE project_id=? AND kind='file' AND key=?",
                (project_id, target_path),
            ).fetchone()
            if target:
                edge(conn, module_row[0], target[0], "RESOLVES_TO", imported["chunk_id"])

    commit_node_ids = {}
    for row in conn.execute(
        "SELECT c.sha,c.parents,c.subject,d.id AS document_id,ch.id AS chunk_id "
        "FROM commits c JOIN documents d ON d.project_id=c.project_id AND d.commit_sha=c.sha "
        "LEFT JOIN chunks ch ON ch.document_id=d.id AND ch.kind='commit' "
        "WHERE c.project_id=?", (project_id,),
    ):
        commit_node_ids[row["sha"]] = node(conn, project_id, "commit", row["sha"],
                                            f"{row['sha'][:12]} {row['subject']}", row["document_id"], row["chunk_id"])
    for row in conn.execute("SELECT commit_sha,path,status,chunk_id FROM changes WHERE project_id=?", (project_id,)):
        file_node = node(conn, project_id, "file", row["path"], row["path"])
        commit_id = commit_node_ids.get(row["commit_sha"])
        if commit_id:
            edge(conn, file_node, commit_id, "CHANGED_BY", row["chunk_id"])
    for row in conn.execute("SELECT ref_name,kind,commit_sha FROM branches WHERE project_id=?", (project_id,)):
        branch_id = node(conn, project_id, "branch", row["ref_name"], row["ref_name"])
        commit_id = commit_node_ids.get(row["commit_sha"])
        if commit_id:
            edge(conn, branch_id, commit_id, "POINTS_TO")
    project_state = conn.execute("SELECT git_head,git_ref FROM projects WHERE id=?", (project_id,)).fetchone()
    if project_state["git_head"]:
        head_id = node(conn, project_id, "branch", "HEAD",
                       f"HEAD ({project_state['git_ref'] or 'unknown'})")
        commit_id = commit_node_ids.get(project_state["git_head"])
        if commit_id:
            edge(conn, head_id, commit_id, "POINTS_TO")
    for row in conn.execute("SELECT sha,parents FROM commits WHERE project_id=?", (project_id,)):
        child = commit_node_ids.get(row["sha"])
        if not child:
            continue
        for parent_sha in row["parents"].split():
            parent = commit_node_ids.get(parent_sha)
            if parent:
                edge(conn, child, parent, "PARENT_COMMIT")


def module_file_candidates(source_path: str, language: str | None, module: str):
    """Resolve only conventional, exact local import paths; unresolved imports stay as module nodes."""
    candidates = set()
    source = module.strip().strip("'\"")
    current = PurePosixPath(source_path)
    if language in {"javascript", "typescript", "tsx"} and source.startswith("."):
        base = PurePosixPath(os.path.normpath(str(current.parent / source)))
        if base.suffix:
            candidates.add(base.as_posix())
        else:
            for suffix in (".ts", ".tsx", ".js", ".jsx", ".mjs", ".json"):
                candidates.add(base.as_posix() + suffix)
                candidates.add((base / ("index" + suffix)).as_posix())
    elif language == "python":
        dots = len(source) - len(source.lstrip("."))
        module_name = source[dots:]
        bases = [PurePosixPath("src"), PurePosixPath(".")] if dots == 0 else [current.parent]
        if dots > 1:
            for _ in range(dots - 1):
                bases = [bases[0].parent]
        parts = tuple(module_name.split(".")) if module_name else ()
        for base in bases:
            target = base.joinpath(*parts)
            candidates.add((target.as_posix() + ".py").lstrip("./"))
            candidates.add((target / "__init__.py").as_posix().lstrip("./"))
    elif language == "rust" and source.startswith("crate::"):
        module_path = source[7:].split("{", 1)[0].removesuffix("::").strip()
        parts = tuple(part.strip() for part in module_path.split("::") if part.strip())
        if parts:
            source_parts = current.parts
            if "src" in source_parts:
                src_index = len(source_parts) - 1 - tuple(reversed(source_parts)).index("src")
                source_root = PurePosixPath(*source_parts[:src_index + 1])
                target = source_root.joinpath(*parts)
                candidates.add((target.as_posix() + ".rs").lstrip("./"))
                candidates.add((target / "mod.rs").as_posix().lstrip("./"))
    return sorted(path for path in candidates if path and path != source_path and not path.startswith("../"))


def refresh_project(conn: sqlite3.Connection, project: sqlite3.Row, parser_api,
                     max_file_bytes: int | None = None, force_parse: bool = False):
    root = Path(project["root"])
    if not root.is_dir():
        raise RuntimeError(f"프로젝트 디렉터리가 없습니다: {root}")
    max_bytes = max_file_bytes or project["max_file_bytes"] or MAX_FILE_BYTES
    project_id = project["id"]
    git_dir = git_root(root)
    conn.execute("CREATE TEMP TABLE IF NOT EXISTS seen_files(path TEXT PRIMARY KEY) WITHOUT ROWID")
    conn.execute("DELETE FROM seen_files")
    counts = {"added": 0, "updated": 0, "deleted": 0, "unchanged": 0, "missing": 0,
              "unreadable": 0, "skipped": 0, "parse_errors": 0}
    changed = False
    incomplete = False
    for rel in iter_files(root, git_dir):
        outcome = add_file(conn, project_id, root, rel, max_bytes, parser_api, force_parse)
        if outcome in {"added", "updated", "unchanged"}:
            conn.execute("INSERT OR IGNORE INTO seen_files(path) VALUES(?)", (rel,))
        if outcome == "unreadable":
            incomplete = True
        if outcome in counts:
            counts[outcome] += 1
        else:
            counts["skipped"] += 1
        if outcome in {"added", "updated"}:
            changed = True
            row = conn.execute(
                "SELECT parse_status FROM documents WHERE project_id=? AND source_kind='file' AND path=?",
                (project_id, rel),
            ).fetchone()
            if row and row[0] in {"error", "unsupported"}:
                counts["parse_errors"] += 1
    stale_chunks = [] if incomplete else conn.execute(
        "SELECT c.id FROM chunks c JOIN documents d ON d.id=c.document_id "
        "WHERE d.project_id=? AND d.source_kind='file' AND NOT EXISTS "
        "(SELECT 1 FROM seen_files s WHERE s.path=d.path)", (project_id,),
    ).fetchall()
    for row in stale_chunks:
        conn.execute("DELETE FROM chunk_fts WHERE rowid=?", (row[0],))
    removed = 0 if incomplete else conn.execute(
        "DELETE FROM documents WHERE project_id=? AND source_kind='file' AND NOT EXISTS "
        "(SELECT 1 FROM seen_files s WHERE s.path=documents.path)", (project_id,),
    ).rowcount
    counts["deleted"] = max(0, removed)
    changed = changed or removed > 0
    facts = git_facts(root)
    git_changed = sync_git(conn, project_id, root, facts)
    changed = changed or git_changed
    if changed:
        rebuild_graph(conn, project_id)
    conn.execute("UPDATE projects SET indexed_at=?,max_file_bytes=? WHERE id=?", (int(time.time()), max_bytes, project_id))
    conn.commit()
    return counts, changed


def expand_query(query: str) -> list[str]:
    raw_tokens = re.findall(r"[\w]+", query, re.UNICODE)
    terms = []
    seen = set()

    def add(value: str):
        value = value.strip().lower()
        if len(value) < 2 or value in seen:
            return
        seen.add(value)
        terms.append(value)

    for token in raw_tokens:
        lookup = token.lower()
        add(token)
        for part in re.split(r"[_\-./:]+", token):
            add(part)
        for part in re.sub(r"([a-z0-9])([A-Z])", r"\1 \2", token).split():
            add(part)
        for suffix in KOREAN_PARTICLES:
            if token.endswith(suffix) and len(token) > len(suffix) + 1:
                lookup = token[:-len(suffix)].lower()
                add(lookup)
                break
        for expanded in QUERY_EXPANSIONS.get(lookup, ()):
            add(expanded)
    return terms[:24]


def fts_query(terms: list[str], original: str) -> str:
    if not terms:
        return ""
    escaped = ['"' + term.replace('"', '""') + '"' for term in terms]
    phrase = '"' + original.replace('"', '""') + '"'
    return phrase + " OR " + " OR ".join(escaped)


def find_project(conn: sqlite3.Connection, value: str | None):
    if value is None:
        return None
    row = conn.execute("SELECT * FROM projects WHERE root=? OR name=? ORDER BY root=? DESC LIMIT 2",
                       (str(Path(value).expanduser().resolve()), value, str(Path(value).expanduser().resolve()))).fetchall()
    if not row:
        raise RuntimeError(f"색인된 프로젝트를 찾지 못했습니다: {value}")
    if len(row) > 1:
        raise RuntimeError(f"프로젝트 이름이 중복됩니다. --project에 절대 경로를 지정하세요: {value}")
    return row[0]


def read_current_evidence(conn: sqlite3.Connection, row: sqlite3.Row, warnings: list[str]):
    project_root = Path(row["root"])
    source_kind = row["source_kind"]
    if source_kind == "git":
        sha = row["commit_sha"]
        return {
            "project": row["project_name"], "source": "git", "path": row["path"],
            "kind": row["chunk_kind"], "symbol": row["symbol"],
            "commit": sha, "author": row["author"], "authored_at": row["authored_at"],
            "start_line": None, "end_line": None,
            "excerpt": redact_secrets(row["body"])[:MAX_CHUNK_CHARS],
        }
    rel = row["path"]
    try:
        absolute = project_root / Path(rel)
        if not absolute.resolve().is_relative_to(project_root.resolve()):
            warnings.append(f"프로젝트 경로 밖이라 제외함: {rel}")
            return None
        raw = absolute.read_bytes()
    except OSError:
        warnings.append(f"원본 파일이 없어 제외함: {rel}")
        return None
    digest = hashlib.sha256(raw).hexdigest()
    if digest != row["source_sha256"]:
        warnings.append(f"색인 직후 원본이 다시 바뀌어 제외함: {rel}")
        return None
    lines = raw.decode("utf-8-sig", "replace").splitlines()
    start = row["start_line"] or 1
    end = row["end_line"] or min(len(lines), start + 39)
    excerpt = redact_secrets("\n".join(lines[max(0, start - 1):max(start, end)]))
    return {
        "project": row["project_name"], "source": "file", "path": rel,
        "kind": row["chunk_kind"], "symbol": row["symbol"],
        "document_kind": row["document_kind"], "parse_status": row["parse_status"],
        "start_line": start, "end_line": min(end, len(lines)), "sha256": digest,
        "excerpt": excerpt[:MAX_CHUNK_CHARS],
    }


def graph_expansion(conn: sqlite3.Connection, project_id: int, seed_rows, direct_ids: set[int]):
    allowed = ("DECLARES", "IMPORTS", "CONTAINS_SECTION", "LINKS_TO", "DOCUMENTED_IN",
               "MENTIONED_IN", "MENTIONS_SYMBOL", "HAS_TODO", "CHANGED_BY", "PARENT_COMMIT",
               "POINTS_TO", "RESOLVES_TO")
    placeholders = ",".join("?" for _ in allowed)
    queue = []
    for row in seed_rows[:8]:
        for node_row in conn.execute(
            "SELECT id FROM nodes WHERE project_id=? AND (chunk_id=? OR document_id=?)",
            (project_id, row["chunk_id"], row["document_id"]),
        ):
            queue.append((node_row[0], [], 0))
    visited = set()
    found = []
    found_ids = set(direct_ids)
    while queue and len(visited) < MAX_GRAPH_NODES and len(found) < MAX_GRAPH_EVIDENCE:
        node_id, route, depth = queue.pop(0)
        if node_id in visited or depth >= 2:
            continue
        visited.add(node_id)
        edges = conn.execute(
            f"SELECT e.relation,e.evidence_chunk_id,n.id AS neighbor_id,n.kind AS neighbor_kind,"
            f"n.label,n.document_id,n.chunk_id,d.authored_at "
            f"FROM edges e JOIN nodes n ON n.id=CASE WHEN e.source_id=? THEN e.target_id ELSE e.source_id END "
            f"LEFT JOIN documents d ON d.id=n.document_id "
            f"WHERE (e.source_id=? OR e.target_id=?) AND e.relation IN ({placeholders}) "
            "ORDER BY d.authored_at DESC LIMIT 20",
            (node_id, node_id, node_id, *allowed),
        ).fetchall()
        for item in edges:
            neighbor_id = item["neighbor_id"]
            relation = {"node": item["label"], "relation": item["relation"]}
            next_route = route + [relation]
            evidence_chunk = item["evidence_chunk_id"] or item["chunk_id"]
            if item["neighbor_kind"] == "file" and item["document_id"]:
                representative = conn.execute(
                    "SELECT id FROM chunks WHERE document_id=? ORDER BY ordinal LIMIT 1",
                    (item["document_id"],),
                ).fetchone()
                if representative:
                    evidence_chunk = representative[0]
            if evidence_chunk and evidence_chunk not in found_ids:
                hit = conn.execute(
                    "SELECT c.id AS chunk_id,c.document_id,c.kind AS chunk_kind,c.symbol,c.start_line,c.end_line,c.body,"
                    "d.project_id,d.source_kind,d.path,d.kind AS document_kind,d.parse_status,"
                    "d.sha256 AS source_sha256,d.commit_sha,d.author,d.authored_at,"
                    "p.root,p.name AS project_name "
                    "FROM chunks c JOIN documents d ON d.id=c.document_id JOIN projects p ON p.id=d.project_id "
                    "WHERE c.id=? AND d.project_id=?", (evidence_chunk, project_id),
                ).fetchone()
                if hit:
                    found.append((hit, next_route))
                    found_ids.add(evidence_chunk)
            if neighbor_id not in visited and len(visited) + len(queue) < MAX_GRAPH_NODES:
                queue.append((neighbor_id, next_route, depth + 1))
            if len(found) >= MAX_GRAPH_EVIDENCE:
                break
    return found


def search(conn: sqlite3.Connection, query: str, project_value: str | None, limit: int, parser_api):
    terms = expand_query(query)
    projects = [find_project(conn, project_value)] if project_value else conn.execute("SELECT * FROM projects ORDER BY root").fetchall()
    if not projects:
        return {"query": query, "expanded_terms": terms, "evidence": [], "warnings": ["아직 색인한 프로젝트가 없습니다."]}
    for project in projects:
        refresh_project(conn, project, parser_api)
    match = fts_query(terms, query)
    if not match:
        return {"query": query, "expanded_terms": terms, "evidence": [], "warnings": ["검색어에서 검색 가능한 토큰을 찾지 못했습니다."]}
    project_ids = [p["id"] for p in projects]
    placeholders = ",".join("?" for _ in project_ids)
    rows = conn.execute(
        f"SELECT c.id AS chunk_id,c.document_id,c.kind AS chunk_kind,c.symbol,c.start_line,c.end_line,c.body,"
        f"d.project_id,d.source_kind,d.path,d.kind AS document_kind,d.parse_status,"
        f"d.sha256 AS source_sha256,d.commit_sha,d.author,d.authored_at,"
        f"p.root,p.name AS project_name,bm25(chunk_fts,0.5,0.3,2.0,1.0) AS score "
        f"FROM chunk_fts JOIN chunks c ON c.id=chunk_fts.rowid "
        f"JOIN documents d ON d.id=c.document_id JOIN projects p ON p.id=d.project_id "
        f"WHERE chunk_fts MATCH ? AND d.project_id IN ({placeholders}) "
        "ORDER BY score LIMIT ?", (match, *project_ids, max(limit * 3, 24)),
    ).fetchall()
    direct = rows[:limit]
    direct_ids = {r["chunk_id"] for r in direct}
    related = []
    for project in projects:
        seeds = [r for r in rows if r["project_id"] == project["id"]]
        related.extend((row, route) for row, route in graph_expansion(conn, project["id"], seeds, direct_ids))
    warnings = []
    unsupported = sum(
        conn.execute(
            "SELECT COUNT(*) FROM documents WHERE project_id=? AND source_kind='file' "
            "AND kind='code' AND parse_status IN ('error','unsupported')", (project["id"],),
        ).fetchone()[0]
        for project in projects
    )
    if unsupported:
        warnings.append(f"{unsupported}개 코드 파일은 AST 분석이 없으며, 텍스트 검색만 색인했습니다.")
    evidence = []
    seen = set()
    for row in direct:
        item = read_current_evidence(conn, row, warnings)
        if item:
            item["match"] = "fts"
            evidence.append(item)
            seen.add(row["chunk_id"])
    for row, route in related:
        if row["chunk_id"] in seen:
            continue
        item = read_current_evidence(conn, row, warnings)
        if item:
            item["match"] = "graph"
            item["via"] = route
            evidence.append(item)
            seen.add(row["chunk_id"])
        if len(evidence) >= limit + MAX_GRAPH_EVIDENCE:
            break
    return {"query": query, "expanded_terms": terms, "evidence": evidence,
            "warnings": warnings, "answer": None}


def status(conn: sqlite3.Connection):
    projects = []
    for project in conn.execute("SELECT * FROM projects ORDER BY root"):
        counts = conn.execute(
            "SELECT source_kind,kind,COUNT(*) AS n FROM documents WHERE project_id=? GROUP BY source_kind,kind",
            (project["id"],),
        ).fetchall()
        parse = conn.execute(
            "SELECT parse_status,COUNT(*) AS n FROM documents WHERE project_id=? AND source_kind='file' AND kind='code' GROUP BY parse_status",
            (project["id"],),
        ).fetchall()
        commits = conn.execute("SELECT COUNT(*) FROM commits WHERE project_id=?", (project["id"],)).fetchone()[0]
        branches = conn.execute("SELECT COUNT(*) FROM branches WHERE project_id=?", (project["id"],)).fetchone()[0]
        projects.append({"root": project["root"], "name": project["name"], "indexed_at": project["indexed_at"],
                         "documents": [dict(row) for row in counts], "code_parsers": [dict(row) for row in parse],
                         "commits": commits, "branches": branches, "head": project["git_head"], "ref": project["git_ref"]})
    return {"database": str(conn.execute("PRAGMA database_list").fetchone()[2]), "projects": projects}


def print_result(value, format_name: str):
    if format_name == "json":
        print(json.dumps(value, ensure_ascii=False, indent=2))
        return
    if "projects" in value:
        print(f"DB: {value['database']}")
        for p in value["projects"]:
            print(f"\n{p['root']}  HEAD={p['head'] or '-'} ({p['ref'] or '-'})")
            print(f"  commits={p['commits']} branches={p['branches']}")
            print(f"  documents={p['documents']}")
            print(f"  code parsers={p['code_parsers']}")
        return
    if "added" in value:
        print(json.dumps(value, ensure_ascii=False, indent=2))
        return
    print(f"질문: {value['query']}")
    print(f"확장 검색어: {', '.join(value['expanded_terms'])}")
    if not value["evidence"]:
        print("근거를 찾지 못했습니다.")
    for i, item in enumerate(value["evidence"], 1):
        cite = f"{item['path']}:{item['start_line']}-{item['end_line']}" if item["source"] == "file" else f"commit {item['commit']}"
        print(f"\n[{i}] {cite}  ({item['match']}; {item['kind']})")
        print(item["excerpt"])
        if item.get("via"):
            print("관계: " + " → ".join(f"{step['relation']} {step['node']}" for step in item["via"]))
    for warning in value["warnings"]:
        print(f"경고: {warning}")


def main(argv=None):
    parser = argparse.ArgumentParser(description="SQLite FTS5 + Tree-sitter + local Git evidence search")
    parser.add_argument("--db", type=Path, default=default_db_path(), help="SQLite DB 경로")
    commands = parser.add_subparsers(dest="command", required=True)
    index_cmd = commands.add_parser("index", help="프로젝트를 색인하거나 갱신")
    index_cmd.add_argument("root", type=Path)
    index_cmd.add_argument("--max-file-bytes", type=int, default=MAX_FILE_BYTES)
    index_cmd.add_argument("--force-parse", action="store_true", help="변경되지 않은 코드도 Tree-sitter로 다시 분석")
    search_cmd = commands.add_parser("search", help="현재 원본 검증이 포함된 근거 묶음 검색")
    search_cmd.add_argument("query")
    search_cmd.add_argument("--project", help="프로젝트 절대 경로 또는 고유 이름")
    search_cmd.add_argument("--limit", type=int, default=12)
    search_cmd.add_argument("--format", choices=("json", "text"), default="json")
    status_cmd = commands.add_parser("status", help="색인한 프로젝트 요약")
    status_cmd.add_argument("--format", choices=("json", "text"), default="text")
    args = parser.parse_args(argv)
    try:
        conn = connect_db(args.db)
        if args.command == "status":
            print_result(status(conn), args.format)
        elif args.command == "index":
            if args.max_file_bytes < 1:
                raise RuntimeError("--max-file-bytes는 1 이상이어야 합니다.")
            parser_api = need_treesitter()
            root = args.root.expanduser().resolve()
            if not root.is_dir():
                raise RuntimeError(f"프로젝트 디렉터리가 아닙니다: {root}")
            project_id = ensure_project(conn, root, args.max_file_bytes)
            project = conn.execute("SELECT * FROM projects WHERE id=?", (project_id,)).fetchone()
            counts, _ = refresh_project(conn, project, parser_api, args.max_file_bytes, args.force_parse)
            print(json.dumps({"project": str(root), **counts}, ensure_ascii=False, indent=2))
        else:
            if not 1 <= args.limit <= MAX_RESULTS:
                raise RuntimeError(f"--limit은 1~{MAX_RESULTS} 사이여야 합니다.")
            parser_api = need_treesitter()
            print_result(search(conn, args.query, args.project, args.limit, parser_api), args.format)
        conn.close()
        return 0
    except (RuntimeError, sqlite3.Error, OSError) as exc:
        print(f"project-knowledge: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
