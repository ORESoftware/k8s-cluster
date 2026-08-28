export function createBraveProvider({ apiKey, fetchJson }) {
  return {
    name: 'brave',
    kind: 'search',
    configured: Boolean(apiKey),
    async search(query, count) {
      if (!apiKey) return [];
      const url = new URL('https://api.search.brave.com/res/v1/web/search');
      url.searchParams.set('q', query);
      url.searchParams.set('count', String(Math.min(count, 20)));
      const body = await fetchJson(url, {
        headers: {
          Accept: 'application/json',
          'X-Subscription-Token': apiKey,
        },
      });
      return (Array.isArray(body?.web?.results) ? body.web.results : [])
        .map((item) => item?.url)
        .filter(Boolean);
    },
  };
}
