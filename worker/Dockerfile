# The myoushq worker: the Python client, Playwright's Chromium, and a VNC
# view of the browser so the owner can log into sites once.
# Build context is the repository root (see compose.yml).
# Base image pinned by digest; bump the tag and digest together.
FROM mcr.microsoft.com/playwright/python:v1.63.0-noble@sha256:72bd171a9ffc2b4b59532aaa6210e21014d07093120dc25528870c0b840da1f0

# Virtual display, VNC server, and noVNC (the VNC client as a web page).
RUN apt-get update \
	&& apt-get install -y --no-install-recommends tigervnc-standalone-server matchbox-window-manager novnc websockify python3-venv \
	&& rm -rf /var/lib/apt/lists/*

# The client, from the verified checkout, dependencies pinned by hash
# (docs/supply-chain.md), plus the Playwright package matching the image's
# browsers (the image ships the browsers, not the Python package).
COPY python /src/python
RUN python3 -m venv --system-site-packages /opt/venv \
	&& /opt/venv/bin/pip install --no-cache-dir --require-hashes -r /src/python/requirements.lock \
	&& /opt/venv/bin/pip install --no-cache-dir --no-deps /src/python \
	&& /opt/venv/bin/pip install --no-cache-dir playwright==1.63.0 \
	&& rm -rf /src/python

# Non-root. /home/worker/.myous (identity, state) and /home/worker/profile
# (browser logins) are volumes; /work is where requests run.
RUN useradd -m -u 10001 worker \
	&& mkdir -p /work /home/worker/profile /home/worker/.myous \
	&& chown -R worker:worker /work /home/worker
COPY --chown=worker:worker worker/browser.py worker/entrypoint.sh worker/review.py /opt/worker/
RUN chmod 755 /opt/worker/entrypoint.sh /opt/worker/review.py

USER worker
ENV PATH=/opt/venv/bin:$PATH \
	HOME=/home/worker \
	MYOUS_HOME=/home/worker/.myous \
	MYOUS_BROWSER_PROFILE=/home/worker/profile \
	MYOUS_BROWSER_NO_SANDBOX=1 \
	DISPLAY=:99
WORKDIR /work
EXPOSE 6080
ENTRYPOINT ["/opt/worker/entrypoint.sh"]
