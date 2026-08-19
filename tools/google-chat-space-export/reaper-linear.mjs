import { fetchJsonWithRetry } from './reaper-http.mjs';
import { normalizeIssue } from './reaper-provenance.mjs';
import { candidateMarker } from './reaper-utils.mjs';

const LINEAR_ENDPOINT = 'https://api.linear.app/graphql';

export function createLinearClient({ apiKey, teamId, parentIssue, fetchImpl } = {}) {
  if (!apiKey) throw new Error('LINEAR_API_KEY is required');
  const headers = {
    authorization: apiKey,
    'content-type': 'application/json',
    'user-agent': 'google-chat-reaper/1',
  };
  async function graphql(query, variables = {}) {
    const payload = await fetchJsonWithRetry(
      LINEAR_ENDPOINT,
      { method: 'POST', headers, body: JSON.stringify({ query, variables }) },
      { fetchImpl },
    );
    if (Array.isArray(payload.errors) && payload.errors.length) {
      throw new Error(`Linear GraphQL error: ${payload.errors.map((item) => item.message).join('; ')}`);
    }
    return payload.data;
  }

  let parentUuidPromise;
  async function parentUuid() {
    if (!parentIssue) return null;
    if (!parentUuidPromise) {
      parentUuidPromise = graphql(
        `query ReaperParent($id: String!) { issue(id: $id) { id identifier } }`,
        { id: parentIssue },
      ).then((data) => data.issue?.id || null);
    }
    return parentUuidPromise;
  }

  async function exportIndex() {
    const issues = [];
    let after = null;
    do {
      const data = await graphql(
        `query ReaperIndex($after: String, $teamId: ID!) {
          issues(
            first: 100,
            after: $after,
            includeArchived: true,
            orderBy: updatedAt,
            filter: { team: { id: { eq: $teamId } } }
          ) {
            nodes {
              id identifier title description url
              state { name type }
              project { name }
            }
            pageInfo { hasNextPage endCursor }
          }
        }`,
        { after, teamId },
      );
      for (const issue of data.issues.nodes) issues.push(normalizeIssue(issue));
      after = data.issues.pageInfo.hasNextPage ? data.issues.pageInfo.endCursor : null;
    } while (after);
    return { issues };
  }

  async function getIssue(identifier) {
    const data = await graphql(
      `query ReaperIssue($id: String!) {
        issue(id: $id) {
          id identifier title description url
          state { name type }
          project { name }
        }
      }`,
      { id: identifier },
    );
    return data.issue ? normalizeIssue(data.issue) : null;
  }

  async function findByCandidate(candidateKey, title) {
    const data = await graphql(
      `query ReaperFind($teamId: ID!, $candidateKey: String!, $title: String!) {
        issues(
          first: 25,
          includeArchived: true,
          filter: {
            team: { id: { eq: $teamId } }
            or: [
              { description: { contains: $candidateKey } }
              { title: { eqIgnoreCase: $title } }
            ]
          }
        ) {
          nodes {
            id identifier title description url
            state { name type }
            project { name }
          }
        }
      }`,
      { teamId, candidateKey, title },
    );
    return data.issues.nodes.map(normalizeIssue);
  }

  async function createIssue({ title, description }) {
    const data = await graphql(
      `mutation ReaperCreate($input: IssueCreateInput!) {
        issueCreate(input: $input) {
          success
          issue {
            id identifier title description url
            state { name type }
            project { name }
          }
        }
      }`,
      {
        input: {
          teamId,
          title,
          description,
          parentId: await parentUuid(),
          priority: 3,
        },
      },
    );
    if (!data.issueCreate.success || !data.issueCreate.issue) {
      throw new Error('Linear issueCreate returned success=false');
    }
    return normalizeIssue(data.issueCreate.issue);
  }

  async function appendSection(issue, section) {
    if (issue.description.includes(candidateMarker(section.candidateKey))) return issue;
    const existing = issue.description.trim();
    const description = `${existing}${existing ? '\n\n---\n\n' : ''}${section.body}`;
    const data = await graphql(
      `mutation ReaperUpdate($id: String!, $input: IssueUpdateInput!) {
        issueUpdate(id: $id, input: $input) {
          success
          issue {
            id identifier title description url
            state { name type }
            project { name }
          }
        }
      }`,
      { id: issue.id, input: { description } },
    );
    if (!data.issueUpdate.success || !data.issueUpdate.issue) {
      throw new Error(`Linear issueUpdate returned success=false for ${issue.identifier}`);
    }
    return normalizeIssue(data.issueUpdate.issue);
  }

  return { exportIndex, getIssue, findByCandidate, createIssue, appendSection };
}
