# The digest pins the multi-platform Python 3.8.20 slim-bookworm image.
FROM python:3.8.20-slim-bookworm@sha256:1d52838af602b4b5a831beb13a0e4d073280665ea7be7f69ce2382f29c5a613f

# Resolve all OS packages from a signed, dated Debian snapshot.
RUN rm -f /etc/apt/sources.list.d/debian.sources \
    && printf '%s\n' \
      'deb [check-valid-until=no] https://snapshot.debian.org/archive/debian/20261103T000000Z bookworm main' \
      'deb [check-valid-until=no] https://snapshot.debian.org/archive/debian/20261103T000000Z bookworm-updates main' \
      'deb [check-valid-until=no] https://snapshot.debian.org/archive/debian-security/20261103T000000Z bookworm-security main' \
      > /etc/apt/sources.list \
    && apt-get update
# The current Debian mirror is faster; accept a package from it only when its
# size and SHA-256 match the signed snapshot. Fall back to the snapshot URL.
COPY fetch_debs.py /tmp/fetch_debs.py
RUN apt-get -o Acquire::ForceHash=sha256 --print-uris -y --no-install-recommends install \
      build-essential=12.9 libmemcached-dev=1.1.4-1 memcached=1.6.18-1+deb12u1 \
      > /tmp/apt-uris \
    && python /tmp/fetch_debs.py < /tmp/apt-uris \
    && apt-get --no-download -y --no-install-recommends install \
      build-essential=12.9 libmemcached-dev=1.1.4-1 memcached=1.6.18-1+deb12u1 \
    && rm -rf /var/lib/apt/lists/* /tmp/apt-uris /tmp/fetch_debs.py
RUN pip install --no-cache-dir \
    asgiref==3.3.1 docutils==0.16 jinja2==2.11.3 markupsafe==1.1.1 \
    pytz==2021.1 pyyaml==5.4.1 six==1.17.0 sqlparse==0.4.1 tblib==1.7.0 \
    python-memcached==1.59 pylibmc==1.6.3
LABEL dev.jouet.bench.django13344="1"
