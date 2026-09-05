FROM debian:bookworm-slim

ARG VERSION
ARG REVISION
ARG SOURCE

LABEL org.opencontainers.image.title="Krit" \
      org.opencontainers.image.description="Human-auditable, capability-scoped agent language" \
      org.opencontainers.image.licenses="Apache-2.0" \
      org.opencontainers.image.version="${VERSION}" \
      org.opencontainers.image.revision="${REVISION}" \
      org.opencontainers.image.source="${SOURCE}"

RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates libgcc-s1 zlib1g \
    && rm -r /var/lib/apt/lists \
    && mkdir /work \
    && chown 65532:65532 /work

COPY package/krit /usr/local/bin/krit
COPY package/LICENSE package/NOTICE package/BUILD.json package/THIRD-PARTY-LICENSES.json /usr/share/doc/krit/
COPY package/licenses/ /usr/share/doc/krit/licenses/
COPY package/krit.pkg /work/krit.pkg
COPY package/examples/ /work/examples/

ENV HOME=/tmp
USER 65532:65532
WORKDIR /work
ENTRYPOINT ["/usr/local/bin/krit"]
CMD ["--help"]
