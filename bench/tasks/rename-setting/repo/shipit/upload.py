"""Uploading one artifact, with retries."""


class UploadError(Exception):
    pass


def upload_with_retries(send, artifact, retry_limit, timeout_s):
    """Call `send(artifact, timeout_s)` up to `retry_limit` times.

    Returns the number of attempts it took. Raises UploadError once
    `retry_limit` attempts have failed.
    """
    last = None
    for attempt in range(1, retry_limit + 1):
        try:
            send(artifact, timeout_s)
            return attempt
        except OSError as e:
            last = e
    raise UploadError(f"{artifact}: failed after {retry_limit} attempts: {last}")
