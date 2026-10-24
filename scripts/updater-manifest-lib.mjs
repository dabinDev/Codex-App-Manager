export function artifactDownloadUrl(artifactBaseUrl, tag, file) {
  return `${artifactBaseUrl}/${encodeURIComponent(tag)}/${encodeURIComponent(file)}`;
}

export function buildUpdaterManifest({
  tag,
  productName,
  artifactBaseUrl,
  pubDate,
  platforms,
}) {
  return {
    version: tag.replace(/^v/, ""),
    notes: `${productName} ${tag}`,
    pub_date: pubDate,
    platforms: Object.fromEntries(
      Object.entries(platforms).map(([key, platform]) => [
        key,
        {
          signature: platform.signature,
          url: artifactDownloadUrl(artifactBaseUrl, tag, platform.bundle),
        },
      ]),
    ),
  };
}
