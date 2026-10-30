"""python3 -m shipit.cli [--retry-limit N] [--timeout-s S] [--print-config] ARTIFACT..."""

import argparse

from . import config
from .client import Client


def parse(argv):
    parser = argparse.ArgumentParser(prog="shipit")
    parser.add_argument("--config", default="shipit.toml")
    parser.add_argument("--retry-limit", type=int, dest="retry_limit")
    parser.add_argument("--timeout-s", type=int, dest="timeout_s")
    parser.add_argument("--print-config", action="store_true")
    parser.add_argument("artifacts", nargs="*")
    return parser.parse_args(argv)


def main(argv=None, send=None):
    args = parse(argv)
    settings = config.load(args.config, {"retry_limit": args.retry_limit, "timeout_s": args.timeout_s})
    client = Client(settings, send or (lambda artifact, timeout_s: None))
    if args.print_config:
        print(client.describe())
        return 0
    for artifact, attempts in client.ship(args.artifacts).items():
        print(f"{artifact}: uploaded after {attempts} attempt(s)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
