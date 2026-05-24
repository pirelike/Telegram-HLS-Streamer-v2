#!/usr/bin/env python3
import argparse
import json
import time
import urllib.error
import urllib.parse
import urllib.request


DEFAULT_URL = "http://127.0.0.1:5050"
OPENER = urllib.request.build_opener(urllib.request.ProxyHandler({}))


def main():
    args = parse_args()
    base_url = args.url.rstrip("/")
    summary = {
        "job_id": args.job_id,
        "runs": args.runs,
        "real": None,
        "virtual": None,
        "telegram": None,
    }

    media_playlist = first_media_playlist(base_url, args.job_id, virtual=False)
    real_segment = first_segment_uri(media_playlist)
    before = get_json(f"{base_url}/api/metrics")
    cold = timed_get(real_segment, args.runs)
    after_cold = get_json(f"{base_url}/api/metrics")
    hot = timed_get(real_segment, args.runs)
    summary["real"] = {
        "segment": real_segment,
        "cold_ms": cold,
        "cache_hit_ms": hot,
    }
    summary["telegram"] = telegram_download_delta(before, after_cold)

    print(f"real segment: {real_segment}")
    print(f"  cold:      {cold:.1f} ms")
    print(f"  cache hit: {hot:.1f} ms")
    if summary["telegram"] is not None:
        telegram = summary["telegram"]
        print(f"  telegram downloads during cold request: {telegram['download_count']}")
        if telegram["download_ms"] is not None:
            print(f"  telegram download time delta: {telegram['download_ms']:.1f} ms")

    virtual_playlist = first_media_playlist(
        base_url,
        args.job_id,
        virtual=True,
        virtual_height=args.virtual_height,
    )
    if virtual_playlist:
        virtual_segment = first_segment_uri(virtual_playlist)
        virtual_ms = timed_get(virtual_segment, args.runs)
        summary["virtual"] = {
            "segment": virtual_segment,
            "latency_ms": virtual_ms,
        }
        print(f"virtual segment: {virtual_segment}")
        print(f"  latency:   {virtual_ms:.1f} ms")
    else:
        print("virtual segment: skipped (no virtual playlist found)")

    print(json.dumps(summary, indent=2, sort_keys=True))
    return 0


def parse_args():
    parser = argparse.ArgumentParser(
        description="Measure THLS playback/cache latency for one completed job."
    )
    parser.add_argument("job_id", help="completed job id")
    parser.add_argument("--url", default=DEFAULT_URL, help=f"THLS base URL (default: {DEFAULT_URL})")
    parser.add_argument("--virtual-height", type=int, help="virtual ABR height to measure, e.g. 720")
    parser.add_argument("--runs", type=int, default=1, help="requests per measurement; reports average")
    return parser.parse_args()


def first_media_playlist(base_url, job_id, virtual=False, virtual_height=None):
    master_url = f"{base_url}/hls/{job_id}/master.m3u8"
    master = get_text(master_url)
    candidates = []
    for line in master.splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        if virtual_height is not None:
            if line == f"video_virtual_{virtual_height}.m3u8":
                return urllib.parse.urljoin(master_url, line)
            continue
        is_virtual = "video_virtual_" in line
        if is_virtual == virtual:
            candidates.append(urllib.parse.urljoin(master_url, line))
    return candidates[0] if candidates else None


def first_segment_uri(playlist_url):
    playlist = get_text(playlist_url)
    for line in playlist.splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        return urllib.parse.urljoin(playlist_url, line)
    raise RuntimeError(f"no segment URI found in {playlist_url}")


def timed_get(url, runs):
    total = 0.0
    for _ in range(max(1, runs)):
        started = time.perf_counter()
        with OPENER.open(url, timeout=120) as resp:
            resp.read()
        total += (time.perf_counter() - started) * 1000.0
    return total / max(1, runs)


def get_text(url):
    try:
        with OPENER.open(url, timeout=30) as resp:
            return resp.read().decode("utf-8")
    except urllib.error.HTTPError as e:
        raise RuntimeError(f"GET {url} failed with HTTP {e.code}: {e.read().decode('utf-8', 'replace')}")


def get_json(url):
    return json.loads(get_text(url))


def telegram_download_delta(before, after):
    try:
        count = int(after["telegram"]["download_count"]) - int(before["telegram"]["download_count"])
        seconds = (
            float(after["telegram"]["download_total_seconds"])
            - float(before["telegram"]["download_total_seconds"])
        )
        return {
            "download_count": count,
            "download_ms": seconds * 1000.0 if count > 0 else None,
        }
    except (KeyError, TypeError, ValueError):
        return None


if __name__ == "__main__":
    raise SystemExit(main())
