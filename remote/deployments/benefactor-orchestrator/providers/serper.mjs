export function createSerperProvider({ apiKey, fetchJson }) {
  return {
    name: 'serper',
    kind: 'search',
    configured: Boolean(apiKey),
    async search(query, count) {
      if (!apiKey) return [];
      const body = await fetchJson('https://google.serper.dev/search', {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
          'X-API-KEY': apiKey,
        },
        body: JSON.stringify({ q: query, num: Math.min(count, 10), gl: 'us', hl: 'en' }),
      });
      return (Array.isArray(body?.organic) ? body.organic : [])
        .map((item) => item?.link)
        .filter(Boolean);
    },
  };
}
