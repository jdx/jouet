"""The client the CLI drives."""

from .upload import upload_with_retries


class Client:
    def __init__(self, settings, send):
        self.retry_limit = settings["retry_limit"]
        self.timeout_s = settings["timeout_s"]
        self.send = send

    def ship(self, artifacts):
        """Upload each artifact; return {artifact: attempts}."""
        return {a: upload_with_retries(self.send, a, self.retry_limit, self.timeout_s) for a in artifacts}

    def describe(self):
        return f"retry_limit={self.retry_limit} timeout_s={self.timeout_s}"
