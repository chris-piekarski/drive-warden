#!/usr/bin/env python3
"""Back up active Google Drive items shared with the operator.

The script reads the local drive-warden SQLite inventory, then downloads or
exports every active item where owned_by_me=false and shared=true. It is
resumable: successful entries in manifest.jsonl are skipped on later runs.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import mimetypes
import os
import shutil
import sqlite3
import sys
import time
from pathlib import Path
from typing import Any, Iterable
from urllib.parse import quote

import requests


RETRYABLE_STATUSES = {408, 429, 500, 502, 503, 504}
CHUNK_SIZE = 1024 * 1024

EXPORTS: dict[str, list[tuple[str, str]]] = {
    "application/vnd.google-apps.document": [
        ("application/vnd.openxmlformats-officedocument.wordprocessingml.document", ".docx"),
        ("application/pdf", ".pdf"),
        ("text/plain", ".txt"),
    ],
    "application/vnd.google-apps.spreadsheet": [
        ("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet", ".xlsx"),
        ("text/csv", ".csv"),
        ("application/pdf", ".pdf"),
        ("application/vnd.oasis.opendocument.spreadsheet", ".ods"),
    ],
    "application/vnd.google-apps.presentation": [
        ("application/vnd.openxmlformats-officedocument.presentationml.presentation", ".pptx"),
        ("application/pdf", ".pdf"),
        ("application/vnd.oasis.opendocument.presentation", ".odp"),
        ("text/plain", ".txt"),
    ],
    "application/vnd.google-apps.drawing": [("application/pdf", ".pdf"), ("image/png", ".png")],
    "application/vnd.google-apps.script": [("application/vnd.google-apps.script+json", ".json")],
    "application/vnd.google-apps.map": [
        ("application/vnd.google-earth.kml+xml", ".kml"),
        ("application/vnd.google-earth.kmz", ".kmz"),
    ],
    "application/vnd.google-apps.earth": [
        ("application/vnd.google-earth.kml+xml", ".kml"),
        ("application/vnd.google-earth.kmz", ".kmz"),
    ],
}

FOLDER_MIME = "application/vnd.google-apps.folder"


def parse_args() -> argparse.Namespace:
    today = dt.datetime.now().strftime("%Y%m%d")
    return argparse.Namespace(**vars(_parser(today).parse_args()))


def _parser(today: str) -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--db", default="data/inventory.db", help="drive-warden SQLite DB")
    parser.add_argument("--credentials", default="data/credentials.json", help="OAuth client JSON")
    parser.add_argument("--tokens", default="data/google-tokens.json", help="yup-oauth2 token cache")
    parser.add_argument(
        "--out",
        default=f"/home/chris/downloads/shared-with-me-backup-{today}",
        help="backup output directory",
    )
    parser.add_argument(
        "--reuse-manifest",
        default="/home/chris/downloads/luxonis-batch-backup/manifest.json",
        help="optional manifest of existing local files to copy instead of redownloading",
    )
    parser.add_argument("--limit", type=int, default=None, help="only process the first N records")
    parser.add_argument("--verbose", action="store_true")
    return parser


def load_shared_items(db_path: Path, limit: int | None) -> list[dict[str, Any]]:
    conn = sqlite3.connect(db_path)
    conn.row_factory = sqlite3.Row
    sql = """
        select
            f.id,
            f.name,
            f.mime_type,
            f.size,
            f.md5_checksum,
            f.modified_time,
            f.permissions_json,
            f.web_view_link,
            pc.primary_path,
            pc.depth,
            pc.path_state
        from files f
        left join path_cache pc on pc.file_id = f.id
        where f.trashed = 0
          and f.owned_by_me = 0
          and f.shared = 1
        order by coalesce(pc.depth, 0), coalesce(pc.primary_path, f.name), f.name
    """
    rows = [dict(row) for row in conn.execute(sql)]
    if limit is not None:
        rows = rows[:limit]
    return rows


def owner_label(item: dict[str, Any]) -> str:
    try:
        permissions = json.loads(item.get("permissions_json") or "[]")
    except json.JSONDecodeError:
        return "(unknown)"
    owners = [
        p.get("email_address") or p.get("display_name") or p.get("id")
        for p in permissions
        if p.get("role") == "owner"
    ]
    return ", ".join([owner for owner in owners if owner]) or "(unknown)"


def sanitize_component(component: str) -> str:
    cleaned = component.replace("\x00", "").strip()
    if cleaned in {"", ".", ".."}:
        cleaned = "_"
    return cleaned[:180]


def output_path(out_dir: Path, item: dict[str, Any], extension: str | None = None) -> Path:
    raw_path = item.get("primary_path") or item["name"]
    parts = [sanitize_component(part) for part in raw_path.strip("/").split("/") if part != ""]
    if not parts:
        parts = [sanitize_component(item["name"])]
    path = out_dir.joinpath(*parts)
    if extension:
        path = path.with_suffix(extension)
    return path


def load_manifest(path: Path) -> dict[str, dict[str, Any]]:
    records: dict[str, dict[str, Any]] = {}
    if not path.exists():
        return records
    with path.open("r", encoding="utf-8") as handle:
        for line in handle:
            line = line.strip()
            if not line:
                continue
            try:
                record = json.loads(line)
            except json.JSONDecodeError:
                continue
            file_id = record.get("id")
            if file_id and record.get("status") in {"downloaded", "exported", "folder", "copied"}:
                records[file_id] = record
    return records


def append_manifest(path: Path, record: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a", encoding="utf-8") as handle:
        handle.write(json.dumps(record, sort_keys=True) + "\n")
        handle.flush()
        os.fsync(handle.fileno())


def load_reuse_files(path: Path) -> tuple[dict[tuple[str, int | None], Path], dict[str, Path]]:
    by_exact: dict[tuple[str, int | None], Path] = {}
    exported_by_stem: dict[str, Path] = {}
    if not path.exists():
        return by_exact, exported_by_stem
    for item in json.loads(path.read_text(encoding="utf-8")):
        local_path = item.get("local_path")
        local_name = item.get("local_name")
        if not local_path or not local_name:
            continue
        source = Path(local_path)
        if not source.exists():
            continue
        size = item.get("local_size")
        by_exact[(local_name.lower(), size)] = source
        if local_name.lower().endswith(".pdf"):
            exported_by_stem[local_name[:-4].lower()] = source
    return by_exact, exported_by_stem


def read_oauth(credentials_path: Path, tokens_path: Path) -> tuple[str, list[dict[str, Any]], dict[str, Any]]:
    credentials = json.loads(credentials_path.read_text(encoding="utf-8"))["installed"]
    tokens = json.loads(tokens_path.read_text(encoding="utf-8"))
    return credentials["token_uri"], tokens, credentials


def parse_expiry(value: Any) -> dt.datetime | None:
    if not value:
        return None
    if isinstance(value, list) and len(value) >= 9:
        try:
            year, ordinal, hour, minute, second, nanosecond, off_h, off_m, off_s = value[:9]
            date = dt.date(int(year), 1, 1) + dt.timedelta(days=int(ordinal) - 1)
            tz = dt.timezone(dt.timedelta(hours=int(off_h), minutes=int(off_m), seconds=int(off_s)))
            return dt.datetime(
                date.year,
                date.month,
                date.day,
                int(hour),
                int(minute),
                int(second),
                int(nanosecond) // 1000,
                tzinfo=tz,
            )
        except (TypeError, ValueError, OverflowError):
            return None
    if not isinstance(value, str):
        return None
    normalized = value.replace("Z", "+00:00")
    try:
        parsed = dt.datetime.fromisoformat(normalized)
    except ValueError:
        return None
    if parsed.tzinfo is None:
        parsed = parsed.replace(tzinfo=dt.timezone.utc)
    return parsed


def expiry_value_from_datetime(value: dt.datetime, previous: Any) -> Any:
    value = value.astimezone(dt.timezone.utc)
    if isinstance(previous, list) and len(previous) >= 9:
        return [
            value.year,
            int(value.strftime("%j")),
            value.hour,
            value.minute,
            value.second,
            value.microsecond * 1000,
            0,
            0,
            0,
        ]
    return value.isoformat().replace("+00:00", "Z")


def select_token(tokens: list[dict[str, Any]]) -> dict[str, Any]:
    preferred = None
    for entry in tokens:
        scopes = set(entry.get("scopes") or [])
        if "https://www.googleapis.com/auth/drive" in scopes:
            preferred = entry
            break
        if "https://www.googleapis.com/auth/drive.readonly" in scopes:
            preferred = entry
    if preferred is None:
        raise RuntimeError("no Drive-capable OAuth token found")
    return preferred


def access_token(credentials_path: Path, tokens_path: Path) -> str:
    token_uri, tokens, credentials = read_oauth(credentials_path, tokens_path)
    entry = select_token(tokens)
    token = entry["token"]
    expiry = parse_expiry(token.get("expires_at"))
    now = dt.datetime.now(dt.timezone.utc)
    if token.get("access_token") and expiry and expiry > now + dt.timedelta(minutes=5):
        return token["access_token"]
    refresh_token = token.get("refresh_token")
    if not refresh_token:
        raise RuntimeError("OAuth token is expired and has no refresh token")
    response = requests.post(
        token_uri,
        data={
            "client_id": credentials["client_id"],
            "client_secret": credentials["client_secret"],
            "refresh_token": refresh_token,
            "grant_type": "refresh_token",
        },
        timeout=30,
    )
    if response.status_code != 200:
        raise RuntimeError(f"OAuth refresh failed: HTTP {response.status_code} {response.text[:500]}")
    payload = response.json()
    token["access_token"] = payload["access_token"]
    expires_in = int(payload.get("expires_in", 3600))
    token["expires_at"] = expiry_value_from_datetime(
        now + dt.timedelta(seconds=expires_in), token.get("expires_at")
    )
    tokens_path.write_text(json.dumps(tokens, indent=2), encoding="utf-8")
    return token["access_token"]


def request_with_retries(url: str, token: str, *, stream: bool) -> requests.Response:
    last_response: requests.Response | None = None
    for attempt in range(6):
        response = requests.get(
            url,
            headers={"Authorization": f"Bearer {token}"},
            timeout=(20, 120),
            stream=stream,
        )
        if response.status_code not in RETRYABLE_STATUSES:
            return response
        last_response = response
        retry_after = response.headers.get("retry-after")
        delay = float(retry_after) if retry_after else min(2**attempt, 30)
        time.sleep(delay)
    assert last_response is not None
    return last_response


def download_to_file(url: str, token: str, target: Path) -> tuple[str, int]:
    target.parent.mkdir(parents=True, exist_ok=True)
    temp = target.with_name(target.name + ".part")
    response = request_with_retries(url, token, stream=True)
    if response.status_code != 200:
        text = response.text[:1000]
        return f"HTTP {response.status_code}: {text}", 0
    bytes_written = 0
    with temp.open("wb") as handle:
        for chunk in response.iter_content(chunk_size=CHUNK_SIZE):
            if not chunk:
                continue
            handle.write(chunk)
            bytes_written += len(chunk)
    temp.replace(target)
    return "", bytes_written


def extension_for_binary(item: dict[str, Any]) -> str | None:
    name = item["name"]
    if Path(name).suffix:
        return None
    guessed = mimetypes.guess_extension(item["mime_type"])
    return guessed


def find_reuse_file(
    item: dict[str, Any],
    exact_reuse: dict[tuple[str, int | None], Path],
    export_reuse: dict[str, Path],
) -> Path | None:
    source = exact_reuse.get((item["name"].lower(), item.get("size")))
    if source is None and item["mime_type"] in EXPORTS:
        source = export_reuse.get(item["name"].lower())
    return source


def iter_progress(items: Iterable[dict[str, Any]]) -> Iterable[tuple[int, dict[str, Any]]]:
    for index, item in enumerate(items, start=1):
        yield index, item


def main() -> int:
    args = parse_args()
    db_path = Path(args.db)
    out_dir = Path(args.out)
    manifest_path = out_dir / "manifest.jsonl"
    summary_path = out_dir / "summary.json"
    credentials_path = Path(args.credentials)
    tokens_path = Path(args.tokens)
    exact_reuse, export_reuse = load_reuse_files(Path(args.reuse_manifest))
    completed = load_manifest(manifest_path)
    items = load_shared_items(db_path, args.limit)
    token = access_token(credentials_path, tokens_path)
    counts: dict[str, int] = {}

    print(f"backup_dir={out_dir}")
    print(f"items={len(items)} already_completed={len(completed)}")

    for index, item in iter_progress(items):
        file_id = item["id"]
        if file_id in completed:
            counts["already_completed"] = counts.get("already_completed", 0) + 1
            continue

        mime_type = item["mime_type"]
        owner = owner_label(item)
        base_record = {
            "id": file_id,
            "name": item["name"],
            "primary_path": item.get("primary_path"),
            "mime_type": mime_type,
            "owner": owner,
            "source_modified_time": item.get("modified_time"),
            "web_view_link": item.get("web_view_link"),
        }

        if mime_type == FOLDER_MIME:
            target = output_path(out_dir, item)
            target.mkdir(parents=True, exist_ok=True)
            record = {**base_record, "status": "folder", "local_path": str(target)}
            append_manifest(manifest_path, record)
            counts["folder"] = counts.get("folder", 0) + 1
            print(f"[{index}/{len(items)}] folder {item.get('primary_path')}")
            continue

        exports = EXPORTS.get(mime_type)
        extension = exports[0][1] if exports else extension_for_binary(item)
        target = output_path(out_dir, item, extension)

        copied_from = find_reuse_file(item, exact_reuse, export_reuse)
        if copied_from is not None:
            target = output_path(out_dir, item, copied_from.suffix or extension)
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(copied_from, target)
            record = {
                **base_record,
                "status": "copied",
                "local_path": str(target),
                "copied_from": str(copied_from),
                "local_size": target.stat().st_size,
            }
            append_manifest(manifest_path, record)
            counts["copied"] = counts.get("copied", 0) + 1
            print(f"[{index}/{len(items)}] copied {item.get('primary_path')}")
            continue

        if exports:
            status = "exported"
        elif mime_type.startswith("application/vnd.google-apps."):
            record = {
                **base_record,
                "status": "skipped",
                "reason": "unsupported Google-native export",
            }
            append_manifest(manifest_path, record)
            counts["skipped"] = counts.get("skipped", 0) + 1
            print(f"[{index}/{len(items)}] skipped unsupported {item.get('primary_path')}")
            continue
        else:
            url = f"https://www.googleapis.com/drive/v3/files/{quote(file_id)}?alt=media"
            status = "downloaded"

        export_mime_used = None
        export_errors = []
        if exports:
            error = ""
            byte_count = 0
            for export_mime, export_extension in exports:
                target = output_path(out_dir, item, export_extension)
                url = (
                    f"https://www.googleapis.com/drive/v3/files/{quote(file_id)}/export"
                    f"?mimeType={quote(export_mime)}"
                )
                error, byte_count = download_to_file(url, token, target)
                if not error:
                    export_mime_used = export_mime
                    break
                export_errors.append(f"{export_mime}: {error}")
            if error:
                error = " | ".join(export_errors)
        else:
            error, byte_count = download_to_file(url, token, target)
        if error:
            record = {**base_record, "status": "error", "reason": error, "local_path": str(target)}
            append_manifest(manifest_path, record)
            counts["error"] = counts.get("error", 0) + 1
            print(f"[{index}/{len(items)}] error {item.get('primary_path')}: {error[:160]}")
            continue

        record = {
            **base_record,
            "status": status,
            "local_path": str(target),
            "local_size": byte_count,
        }
        if export_mime_used:
            record["export_mime_type"] = export_mime_used
        append_manifest(manifest_path, record)
        counts[status] = counts.get(status, 0) + 1
        print(f"[{index}/{len(items)}] {status} {item.get('primary_path')} ({byte_count} bytes)")

    summary = {
        "backup_dir": str(out_dir),
        "manifest": str(manifest_path),
        "total_items": len(items),
        "counts": counts,
        "completed_ids": len(load_manifest(manifest_path)),
        "generated_at": dt.datetime.now(dt.timezone.utc).isoformat(),
    }
    summary_path.write_text(json.dumps(summary, indent=2, sort_keys=True), encoding="utf-8")
    print(json.dumps(summary, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except KeyboardInterrupt:
        print("interrupted", file=sys.stderr)
        raise SystemExit(130)
