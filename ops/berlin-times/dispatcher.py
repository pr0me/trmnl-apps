#!/usr/bin/env python3

import argparse
import json
import os
import re
import sys
import time
import urllib.error
import urllib.request
from datetime import datetime
from urllib.parse import urlparse
from zoneinfo import ZoneInfo


BERLIN = ZoneInfo("Europe/Berlin")
DEFAULT_WORKFLOW = "edition.yml"
EDITIONS = ("morning", "evening")


class DispatcherError(Exception):
    pass


def required_environment(name):
    value = os.environ.get(name, "").strip()
    if not value:
        raise DispatcherError(f"{name} is not configured")
    return value


def validate_repository(repository):
    if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repository):
        raise DispatcherError("github repository is invalid")
    return repository


def validate_edition_url(url):
    parsed = urlparse(url)
    if parsed.scheme != "https" or not parsed.hostname or parsed.username or parsed.password:
        raise DispatcherError("edition url must be credential-free https")
    return url


def edition_slot(edition, now=None):
    current = now or datetime.now(tz=BERLIN)
    return f"{current.astimezone(BERLIN).date().isoformat()}-{edition}"


def published_slot(document):
    edition = document.get("edition_name")
    generated_at = document.get("generated_at")
    if edition not in EDITIONS or not isinstance(generated_at, str):
        raise DispatcherError("published edition metadata is invalid")

    normalized = generated_at.replace("Z", "+00:00")
    normalized = re.sub(r"(\.\d{6})\d+(?=[+-])", r"\1", normalized)
    try:
        generated = datetime.fromisoformat(normalized)
    except ValueError as error:
        raise DispatcherError("published edition timestamp is invalid") from error
    if generated.tzinfo is None:
        raise DispatcherError("published edition timestamp has no timezone")
    return f"{generated.astimezone(BERLIN).date().isoformat()}-{edition}"


def request_json(url, *, headers=None, payload=None):
    body = None if payload is None else json.dumps(payload).encode()
    request = urllib.request.Request(url, data=body, headers=headers or {})
    try:
        with urllib.request.urlopen(request, timeout=20) as response:
            content = response.read()
    except (urllib.error.URLError, TimeoutError) as error:
        raise DispatcherError(f"request failed: {error}") from error

    if not content:
        return None
    try:
        return json.loads(content)
    except json.JSONDecodeError as error:
        raise DispatcherError("response was not valid json") from error


def current_published_slot(url):
    document = request_json(url, headers={"Accept": "application/json"})
    if not isinstance(document, dict):
        raise DispatcherError("published edition response is invalid")
    return published_slot(document)


def dispatch(slot, repository, workflow, ref, token):
    if not token or token == "replace-with-fine-grained-token":
        raise DispatcherError("github token is not configured")
    url = f"https://api.github.com/repos/{repository}/actions/workflows/{workflow}/dispatches"
    headers = {
        "Accept": "application/vnd.github+json",
        "Authorization": f"Bearer {token}",
        "Content-Type": "application/json",
        "User-Agent": "berlin-times-dispatcher",
        "X-GitHub-Api-Version": "2026-03-10",
    }
    request_json(url, headers=headers, payload={"ref": ref, "inputs": {"edition_slot": slot}})


def parse_args():
    parser = argparse.ArgumentParser(description="dispatch a slot-aware Berlin Times edition")
    parser.add_argument("edition", choices=EDITIONS)
    parser.add_argument(
        "--if-stale",
        action="store_true",
        help="dispatch only when the public edition does not match the requested slot",
    )
    return parser.parse_args()


def main():
    args = parse_args()
    slot = edition_slot(args.edition)
    try:
        edition_url = validate_edition_url(required_environment("BERLIN_TIMES_EDITION_URL"))
    except DispatcherError as error:
        print(error, file=sys.stderr)
        return 1
    if args.if_stale:
        try:
            current = current_published_slot(edition_url)
        except DispatcherError as error:
            print(f"could not verify published slot ({error}); dispatching recovery", file=sys.stderr)
        else:
            if current == slot:
                print(f"{slot} is already published")
                return 0
            print(f"published slot is {current}; dispatching recovery for {slot}")

    try:
        repository = validate_repository(
            required_environment("BERLIN_TIMES_GITHUB_REPOSITORY")
        )
    except DispatcherError as error:
        print(error, file=sys.stderr)
        return 1
    workflow = os.environ.get("BERLIN_TIMES_GITHUB_WORKFLOW", DEFAULT_WORKFLOW)
    ref = os.environ.get("BERLIN_TIMES_GITHUB_REF", "main")
    token = os.environ.get("BERLIN_TIMES_GITHUB_TOKEN", "")
    if not token or token == "replace-with-fine-grained-token":
        print("github token is not configured", file=sys.stderr)
        return 1
    last_error = None
    for attempt in range(1, 4):
        try:
            dispatch(slot, repository, workflow, ref, token)
            print(f"dispatched {slot} on attempt {attempt}")
            return 0
        except DispatcherError as error:
            last_error = error
            if attempt < 3:
                time.sleep(2 ** (attempt - 1))
    print(f"could not dispatch {slot}: {last_error}", file=sys.stderr)
    return 1


if __name__ == "__main__":
    sys.exit(main())
