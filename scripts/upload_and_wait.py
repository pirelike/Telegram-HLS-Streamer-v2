#!/usr/bin/env python3
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request


DEFAULT_URL = "http://127.0.0.1:5050"
TERMINAL_STATUSES = {"complete", "error", "cancelled"}
URL_OPENER = urllib.request.build_opener(urllib.request.ProxyHandler({}))


class ApiError(Exception):
    def __init__(self, status, message, headers=None):
        super().__init__(message)
        self.status = status
        self.headers = headers or {}


def main():
    args = parse_args()
    root = Path(__file__).resolve().parents[1]
    video = Path(args.video).expanduser()
    if not video.is_absolute():
        video = (Path.cwd() / video).resolve()

    base_url = args.url.rstrip("/")
    started_server = None
    server_log = None

    try:
        check_video(video)
        health = get_health(base_url)
        if health is None:
            require_tools(["cargo", "ffmpeg", "ffprobe"])
            started_server, server_log = start_server(root)
            print(f"Started THLS with `cargo run` (log: {server_log})")
            health = wait_for_health(base_url, started_server, server_log, args.start_timeout)
        else:
            require_tools(["ffmpeg", "ffprobe"])
            print(f"Using running THLS server at {base_url}")

        require_bots(health)
        job_id = upload_file(base_url, video, args)
        print(f"Queued job: {job_id}")

        status = wait_for_job(base_url, job_id, args.timeout)
        verify_manifest(base_url, job_id)

        master_url = f"{base_url}/hls/{job_id}/master.m3u8"
        print(f"Complete: {status.get('filename', video.name)}")
        print(f"Master playlist: {master_url}")
        return 0
    except KeyboardInterrupt:
        print("Interrupted", file=sys.stderr)
        return 130
    except Exception as e:
        print(f"ERROR: {e}", file=sys.stderr)
        if server_log:
            print(f"Server log: {server_log}", file=sys.stderr)
        return 1
    finally:
        if started_server is not None:
            stop_server(started_server)


def parse_args():
    parser = argparse.ArgumentParser(
        description="Upload a video to THLS, wait for processing, and verify the HLS manifest."
    )
    parser.add_argument("video", help="video file to upload")
    parser.add_argument("--url", default=DEFAULT_URL, help=f"THLS base URL (default: {DEFAULT_URL})")
    parser.add_argument("--timeout", type=int, default=7200, help="seconds to wait for job completion")
    parser.add_argument("--start-timeout", type=int, default=180, help="seconds to wait for server startup")
    parser.add_argument("--title", help="media title stored with the job")
    parser.add_argument("--media-type", default="film", help="media_type metadata value")
    parser.add_argument("--chunk-retries", type=int, default=5, help="per-chunk retry attempts")
    parser.add_argument("--request-timeout", type=int, default=120, help="HTTP request timeout in seconds")
    return parser.parse_args()


def check_video(path):
    if not path.exists():
        raise RuntimeError(f"video file does not exist: {path}")
    if not path.is_file():
        raise RuntimeError(f"video path is not a file: {path}")
    if path.stat().st_size <= 0:
        raise RuntimeError(f"video file is empty: {path}")


def require_tools(names):
    missing = [name for name in names if shutil.which(name) is None]
    if missing:
        raise RuntimeError("missing required tools on PATH: " + ", ".join(missing))


def start_server(root):
    log = tempfile.NamedTemporaryFile(prefix="thls-upload-test-", suffix=".log", delete=False)
    proc = subprocess.Popen(
        ["cargo", "run"],
        cwd=root,
        stdout=log,
        stderr=subprocess.STDOUT,
        stdin=subprocess.DEVNULL,
    )
    log.close()
    return proc, log.name


def stop_server(proc):
    if proc.poll() is not None:
        return
    proc.terminate()
    try:
        proc.wait(timeout=10)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait(timeout=10)


def wait_for_health(base_url, proc, log_path, timeout):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if proc.poll() is not None:
            tail = read_tail(log_path)
            raise RuntimeError(f"server exited before /health was reachable\n{tail}")
        health = get_health(base_url)
        if health is not None:
            return health
        time.sleep(1)
    tail = read_tail(log_path)
    raise RuntimeError(f"server did not become healthy within {timeout}s\n{tail}")


def get_health(base_url):
    try:
        return request_json("GET", f"{base_url}/health", timeout=5)
    except Exception:
        return None


def require_bots(health):
    configured = int(health.get("bots", {}).get("configured", 0))
    if configured <= 0:
        raise RuntimeError("no Telegram bots are configured; set TELEGRAM_BOT_TOKEN_1 and TELEGRAM_CHANNEL_ID_1")


def upload_file(base_url, path, args):
    size = path.stat().st_size
    filename = path.name
    print(f"Uploading {filename} ({format_bytes(size)})")

    init = request_json(
        "POST",
        f"{base_url}/api/upload/init",
        {"filename": filename, "total_size": size},
        timeout=args.request_timeout,
    )
    upload_id = init["upload_id"]
    chunk_size = int(init["chunk_size"])
    total_chunks = int(init["total_chunks"])
    print(f"Upload id: {upload_id}")
    print(f"Chunks: {total_chunks} x {format_bytes(chunk_size)}")

    started = time.monotonic()
    with path.open("rb") as f:
        for index in range(total_chunks):
            chunk = f.read(chunk_size)
            if not chunk:
                raise RuntimeError(f"unexpected EOF at chunk {index}")
            send_chunk(base_url, upload_id, index, chunk, args)
            done = min((index + 1) * chunk_size, size)
            print_progress(index + 1, total_chunks, done, size, started)

    print()
    body = {
        "upload_id": upload_id,
        "metadata": {
            "media_type": args.media_type,
            "title": args.title or path.stem,
            "series_name": "",
            "is_series": False,
            "season_number": None,
            "episode_number": None,
            "part_number": None,
        },
    }
    finalized = request_json(
        "POST",
        f"{base_url}/api/upload/finalize",
        body,
        timeout=args.request_timeout,
    )
    return finalized["job_id"]


def send_chunk(base_url, upload_id, index, chunk, args):
    headers = {
        "X-Upload-Id": upload_id,
        "X-Chunk-Index": str(index),
        "Content-Type": "application/octet-stream",
    }
    for attempt in range(args.chunk_retries):
        try:
            request_json(
                "POST",
                f"{base_url}/api/upload/chunk",
                raw=chunk,
                headers=headers,
                timeout=args.request_timeout,
            )
            return
        except ApiError as e:
            if e.status == 429 and attempt + 1 < args.chunk_retries:
                time.sleep(retry_after_seconds(e, 60))
                continue
            if e.status >= 500 and attempt + 1 < args.chunk_retries:
                time.sleep(2 ** attempt)
                continue
            raise
        except Exception:
            if attempt + 1 >= args.chunk_retries:
                raise
            time.sleep(2 ** attempt)


def wait_for_job(base_url, job_id, timeout):
    deadline = time.monotonic() + timeout
    last_status = None
    while time.monotonic() < deadline:
        status = request_json("GET", f"{base_url}/api/status/{job_id}", timeout=30)
        state = status.get("status")
        description = status.get("description") or ""
        progress = status.get("progress")
        line = f"{state}: {description}"
        if progress is not None:
            line += f" ({round(float(progress))}%)"
        if line != last_status:
            print(line)
            last_status = line
        if state == "complete":
            return status
        if state in TERMINAL_STATUSES:
            raise RuntimeError(status.get("error") or f"job ended with status {state}")
        time.sleep(1.5)
    raise RuntimeError(f"job {job_id} did not complete within {timeout}s")


def verify_manifest(base_url, job_id):
    body = request_bytes("GET", f"{base_url}/hls/{job_id}/master.m3u8", timeout=30)
    text = body.decode("utf-8", errors="replace")
    if "#EXTM3U" not in text:
        raise RuntimeError("master playlist response did not look like an HLS manifest")


def request_json(method, url, body=None, raw=None, headers=None, timeout=120):
    data = None
    final_headers = dict(headers or {})
    if body is not None:
        data = json.dumps(body).encode("utf-8")
        final_headers["Content-Type"] = "application/json"
    elif raw is not None:
        data = raw
    req = urllib.request.Request(url, data=data, method=method, headers=final_headers)
    try:
        with URL_OPENER.open(req, timeout=timeout) as resp:
            payload = resp.read()
    except urllib.error.HTTPError as e:
        message = parse_error_body(e)
        raise ApiError(e.code, message, e.headers) from e
    if not payload:
        return {}
    return json.loads(payload.decode("utf-8"))


def request_bytes(method, url, timeout=120):
    req = urllib.request.Request(url, method=method)
    try:
        with URL_OPENER.open(req, timeout=timeout) as resp:
            return resp.read()
    except urllib.error.HTTPError as e:
        message = parse_error_body(e)
        raise ApiError(e.code, message, e.headers) from e


def parse_error_body(error):
    raw = error.read()
    try:
        parsed = json.loads(raw.decode("utf-8"))
        return parsed.get("message") or parsed.get("error") or f"HTTP {error.code}"
    except Exception:
        text = raw.decode("utf-8", errors="replace").strip()
        return text or f"HTTP {error.code}"


def retry_after_seconds(error, default):
    raw = error.headers.get("Retry-After") if error.headers else None
    if raw:
        try:
            return max(1, int(raw))
        except ValueError:
            pass
    return default


def print_progress(chunks_done, total_chunks, bytes_done, total_bytes, started):
    pct = bytes_done / total_bytes * 100
    elapsed = max(time.monotonic() - started, 0.001)
    speed = bytes_done / elapsed
    sys.stdout.write(
        "\r"
        f"Uploaded {chunks_done}/{total_chunks} chunks "
        f"({pct:.1f}%, {format_bytes(speed)}/s)"
    )
    sys.stdout.flush()


def format_bytes(value):
    units = ["B", "KB", "MB", "GB", "TB"]
    size = float(value)
    for unit in units:
        if size < 1024 or unit == units[-1]:
            return f"{size:.1f} {unit}"
        size /= 1024


def read_tail(path, max_bytes=12000):
    try:
        with open(path, "rb") as f:
            f.seek(0, os.SEEK_END)
            size = f.tell()
            f.seek(max(0, size - max_bytes))
            return f.read().decode("utf-8", errors="replace")
    except OSError:
        return ""


if __name__ == "__main__":
    raise SystemExit(main())
