# Smoke test for myous for Linux. run.sh builds it from a context holding
# this file, smoke.sh and the bundles under fixed names (myous.AppImage,
# myous.deb). Two targets:
#   appimage: the AppImage, extracted (so no FUSE in the container)
#   deb:      the .deb installed with apt, which pulls in its dependencies
FROM debian:bookworm-slim AS base
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
      xvfb x11-utils imagemagick xdotool dbus-x11 fonts-dejavu-core \
      ca-certificates procps \
    && rm -rf /var/lib/apt/lists/*
RUN useradd -m -s /bin/bash tester
COPY smoke.sh /usr/local/bin/smoke.sh
RUN chmod +x /usr/local/bin/smoke.sh

FROM base AS appimage
RUN apt-get update && apt-get install -y --no-install-recommends \
      libwebkit2gtk-4.1-0 libgtk-3-0 libayatana-appindicator3-1 \
    && rm -rf /var/lib/apt/lists/*
COPY myous.AppImage /home/tester/myous.AppImage
RUN cd /home/tester && chmod +x myous.AppImage \
    && ./myous.AppImage --appimage-extract >/dev/null \
    && rm myous.AppImage && chown -R tester /home/tester
USER tester
WORKDIR /home/tester
ENV APP=/home/tester/squashfs-root/AppRun

FROM base AS deb
COPY myous.deb /tmp/myous.deb
RUN apt-get update \
    && apt-get install -y --no-install-recommends /tmp/myous.deb 2>&1 | tee /deb-install.log \
    && dpkg -L myous > /deb-files.txt \
    && rm -rf /var/lib/apt/lists/* /tmp/myous.deb
USER tester
WORKDIR /home/tester
ENV APP=/usr/bin/myous-desktop
