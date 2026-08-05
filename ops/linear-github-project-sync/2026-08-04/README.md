# Linear → GitHub organization project selection — 2026-08-04

This immutable manifest contains the audited next-step selection for all 40 connected GitHub organizations.

Selection policy:

- prefer current Linear work in a started state;
- otherwise select the highest-value current Todo work;
- select at most three issues per organization;
- preserve projects with fewer than three real issues as-is;
- create no placeholder work for projects verified empty across In Progress, Todo, and Backlog.

Totals: 40 organizations, 31 populated projects, 85 selected issues, and 9 verified-empty projects. Linear is the source of truth. The synchronizer creates or reuses GitHub Projects v2 draft items keyed by exact Linear identifier and URL.
