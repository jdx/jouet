from client import Client


def run():
    client = Client("https://example.com")
    return client.fetch("/index.html")
