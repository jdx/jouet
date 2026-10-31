class Client:
    def __init__(self, base):
        self.base = base

    def fetch(self, path):
        return _request(self.base + path)


def _request(url):
    return url
