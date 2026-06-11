// IMPORTANT FOR CODING AGENTS:
// - schema/schema.sql is the final source of truth for database shape.
// - Generated ORM/client files are adapters only; never treat them as migration authority.
// - Never run or apply migrations automatically. Generate SQL for human review and wait for
//   explicit user approval before any database write.
import { readFile } from "node:fs/promises";
import path from "node:path";

const CODEGEN_METADATA = {
  tables: {
    known_git_repos: {
      description: "Git repositories that the remote agent runtime is allowed to clone, run tasks against, and open PRs for.",
      names: {
        typescript: "knownGitRepos",
        rust: "KnownGitRepo",
        gleam: "KnownGitRepo",
      },
      columns: {
        id: {
          generated: true,
        },
        repo_url: {
          validation: {
            minLength: 1,
            maxLength: 2048,
          },
        },
        display_name: {
          validation: {
            minLength: 1,
            maxLength: 200,
          },
        },
        default_branch: {
          validation: {
            minLength: 1,
            maxLength: 120,
          },
        },
      },
    },
    agent_remote_dev_threads: {
      description: "Remote coding-agent chat threads pinned to one git repository and one Kubernetes worker runtime.",
      names: {
        typescript: "agentRemoteDevThreads",
        rust: "AgentRemoteDevThread",
        gleam: "AgentRemoteDevThread",
      },
      columns: {
        title: {
          validation: {
            minLength: 1,
            maxLength: 500,
          },
        },
        repo: {
          validation: {
            minLength: 1,
            maxLength: 2048,
          },
        },
        base_branch: {
          validation: {
            minLength: 1,
            maxLength: 120,
          },
        },
      },
    },
    agent_remote_dev_tasks: {
      description: "Prompt/control tasks dispatched to thread-scoped remote development workers.",
      names: {
        typescript: "agentRemoteDevTasks",
        rust: "AgentRemoteDevTask",
        gleam: "AgentRemoteDevTask",
      },
      columns: {
        prompt: {
          validation: {
            minLength: 1,
            maxBytes: 1048576,
          },
        },
      },
    },
    agent_remote_dev_events: {
      description: "Append-only ordered event stream emitted by remote agent workers.",
      names: {
        typescript: "agentRemoteDevEvents",
        rust: "AgentRemoteDevEvent",
        gleam: "AgentRemoteDevEvent",
      },
      columns: {
        id: {
          generated: true,
        },
        event_kind: {
          validation: {
            minLength: 1,
            maxLength: 80,
          },
        },
      },
    },
    agent_remote_dev_breadcrumbs: {
      description: "Thread-scoped operational breadcrumbs (session-ready, prompt, merge-base, container-pool-result, etc.) used by dev-server to inject prior-task context and by operators to trace worker lifecycle without grepping logs. Postgres is the durable single source of truth for these; Redis (see @dd/redis-interfaces / agent-thread-breadcrumb-cache) is an optional hot-read cache.",
      names: {
        typescript: "agentRemoteDevBreadcrumbs",
        rust: "AgentRemoteDevBreadcrumb",
        gleam: "AgentRemoteDevBreadcrumb",
      },
      columns: {
        id: {
          generated: true,
        },
        kind: {
          validation: {
            minLength: 1,
            maxLength: 80,
          },
        },
        pod_name: {
          validation: {
            maxLength: 253,
          },
        },
        branch: {
          validation: {
            maxLength: 120,
          },
        },
        provider: {
          validation: {
            maxLength: 60,
          },
        },
      },
    },
    agent_remote_dev_artifacts: {
      description: "Artifacts published by remote agent tasks, such as logs, reports, patches, and output files.",
      names: {
        typescript: "agentRemoteDevArtifacts",
        rust: "AgentRemoteDevArtifact",
        gleam: "AgentRemoteDevArtifact",
      },
      columns: {
        id: {
          generated: true,
        },
        filename: {
          validation: {
            minLength: 1,
            maxLength: 1024,
          },
        },
        url: {
          validation: {
            minLength: 1,
            maxLength: 4096,
          },
        },
      },
    },
    agent_remote_dev_runtime_locks: {
      description: "Short-lived leases for queue consumers and reapers that coordinate exactly-one worker ownership per thread.",
      names: {
        typescript: "agentRemoteDevRuntimeLocks",
        rust: "AgentRemoteDevRuntimeLock",
        gleam: "AgentRemoteDevRuntimeLock",
      },
      columns: {
        id: {
          generated: true,
        },
        owner: {
          validation: {
            minLength: 1,
            maxLength: 200,
          },
        },
      },
    },
    lambda_functions: {
      description: "User-defined remote lambda functions executed by the remote runtime.",
      names: {
        typescript: "lambdaFunctions",
        rust: "LambdaFunction",
        gleam: "LambdaFunction",
      },
      columns: {
        id: {
          generated: true,
        },
        display_name: {
          validation: {
            minLength: 1,
          },
        },
        function_body: {
          validation: {
            minLength: 1,
          },
        },
        idle_timeout_seconds: {
          validation: {
            min: 1,
            max: 3600,
          },
        },
        max_run_ms: {
          validation: {
            min: 1000,
            max: 300000,
          },
        },
      },
    },
  },
};

export async function loadSqlContract(packageRoot) {
  const schemaPath = path.join(packageRoot, "schema", "schema.sql");
  const sourceSql = await readFile(schemaPath, "utf8");
  const contract = parseSchemaSql(sourceSql);
  applyMetadata(contract);
  return { contract, sourceSql, schemaPath };
}

export function parseSchemaSql(sourceSql) {
  const statements = splitSqlStatements(sourceSql);
  const tables = [];
  const routines = [];
  const triggers = [];
  const tableByName = new Map();

  for (const statement of statements) {
    const table = parseCreateTable(statement);
    if (!table) {
      continue;
    }

    tables.push(table);
    // Indexes and foreign keys associate to tables by bare name, so two tables sharing a bare
    // name across schemas (e.g. public.foo and benefactor.foo) would silently mis-associate.
    // Fail loudly instead — disambiguating the index/FK `on`/`references` schema is the fix.
    const existing = tableByName.get(table.name);
    if (existing) {
      throw new Error(
        `Duplicate table name "${table.name}" across schemas "${existing.schema}" and "${table.schema}". ` +
          `Index/FK association is keyed by bare table name and cannot disambiguate; rename one table.`,
      );
    }
    tableByName.set(table.name, table);
  }

  for (const statement of statements) {
    const routine = parseCreateFunction(statement);
    if (routine) {
      routines.push(routine);
      continue;
    }

    const trigger = parseCreateTrigger(statement);
    if (trigger) {
      triggers.push(trigger);
      continue;
    }

    const index = parseCreateIndex(statement);
    if (index) {
      const table = tableByName.get(index.tableName);
      if (table) {
        table.indexes.push(index);
      }
      continue;
    }

    const foreignKey = parseForeignKey(statement);
    if (foreignKey) {
      const table = tableByName.get(foreignKey.tableName);
      if (table) {
        table.foreignKeys.push(foreignKey);
        const column = table.columns.find((item) => item.name === foreignKey.column);
        if (column) {
          column.foreignKey = {
            table: foreignKey.references.table,
            column: foreignKey.references.column,
            constraint: foreignKey.name,
          };
        }
      }
    }
  }

  return {
    contractVersion: "sql-source",
    dialect: "postgresql",
    description: "Generated from schema/schema.sql.",
    tables,
    routines,
    triggers,
  };
}

export function splitSqlStatements(sourceSql) {
  const statements = [];
  let current = "";
  let singleQuoted = false;
  let doubleQuoted = false;
  let lineComment = false;
  let dollarQuote = null;

  for (let index = 0; index < sourceSql.length; index += 1) {
    const char = sourceSql[index];
    const next = sourceSql[index + 1];

    if (lineComment) {
      if (char === "\n") {
        lineComment = false;
        current += char;
      }
      continue;
    }

    if (dollarQuote) {
      if (sourceSql.startsWith(dollarQuote, index)) {
        current += dollarQuote;
        index += dollarQuote.length - 1;
        dollarQuote = null;
        continue;
      }

      current += char;
      continue;
    }

    if (!singleQuoted && !doubleQuoted && char === "$") {
      const dollarMatch = sourceSql.slice(index).match(/^\$[A-Za-z0-9_]*\$/);
      if (dollarMatch) {
        dollarQuote = dollarMatch[0];
        current += dollarQuote;
        index += dollarQuote.length - 1;
        continue;
      }
    }

    if (!singleQuoted && !doubleQuoted && char === "-" && next === "-") {
      lineComment = true;
      index += 1;
      continue;
    }

    current += char;

    if (char === "'" && !doubleQuoted) {
      if (singleQuoted && next === "'") {
        current += next;
        index += 1;
        continue;
      }
      singleQuoted = !singleQuoted;
      continue;
    }

    if (char === '"' && !singleQuoted) {
      doubleQuoted = !doubleQuoted;
      continue;
    }

    if (char === ";" && !singleQuoted && !doubleQuoted) {
      const statement = current.trim();
      if (statement) {
        statements.push(statement);
      }
      current = "";
    }
  }

  const trailing = current.trim();
  if (trailing) {
    statements.push(trailing);
  }

  return statements;
}

function parseCreateTable(statement) {
  // Optional `schema.` prefix is captured so tables can live in a non-public Postgres schema
  // (e.g. `benefactor.benefactor_leads`). `name` stays the bare table name; the schema is tracked
  // separately and defaults to `public` so existing unqualified tables are unaffected.
  const match = statement.match(
    /^create\s+table\s+(?:if\s+not\s+exists\s+)?(?:("?[\w]+"?)\.)?("?[\w]+"?)\s*\(([\s\S]*)\)\s*;?$/i,
  );
  if (!match) {
    return null;
  }

  const schema = match[1] ? unquoteIdent(match[1]) : "public";
  const tableName = unquoteIdent(match[2]);
  const body = match[3].trim();
  const columns = [];
  const checks = [];

  for (const item of splitTopLevelComma(body)) {
    const trimmed = item.trim();
    const constraint = parseCheckConstraint(trimmed);
    if (constraint) {
      checks.push(constraint);
      continue;
    }

    const column = parseColumn(trimmed);
    if (column) {
      columns.push(column);
    }
  }

  for (const check of checks) {
    applyCheckValidation(columns, check);
  }

  return {
    name: tableName,
    schema,
    description: "",
    names: {},
    columns,
    checks,
    indexes: [],
    foreignKeys: [],
    createStatement: statement.trim(),
  };
}

function parseForeignKey(statement) {
  // Matches: alter table [if exists] X add constraint Y foreign key (col) references Z(col2)
  // Captures the constraint name, source column, target table, and target column so adapters can
  // expose relationship metadata. Compound (multi-column) FKs are intentionally skipped because
  // the current schema does not use them and a compound FK on a column-level adapter would lie.
  // Both the altered table and the referenced table may carry an optional `schema.` prefix; it is
  // stripped here so association and the exposed `references.table` stay bare table names.
  const match = statement.match(
    /^alter\s+table\s+(?:if\s+exists\s+)?(?:("?[\w]+"?)\.)?("?[\w]+"?)\s+add\s+constraint\s+("?[\w]+"?)\s+foreign\s+key\s*\(\s*("?[\w]+"?)\s*\)\s+references\s+(?:("?[\w]+"?)\.)?("?[\w]+"?)\s*\(\s*("?[\w]+"?)\s*\)\s*;?$/i,
  );
  if (!match) {
    return null;
  }
  return {
    tableName: unquoteIdent(match[2]),
    name: unquoteIdent(match[3]),
    column: unquoteIdent(match[4]),
    references: {
      table: unquoteIdent(match[6]),
      column: unquoteIdent(match[7]),
    },
    statement: statement.trim(),
  };
}

function parseCreateFunction(statement) {
  const bodyMatch = statement.match(/\bas\s+\$([A-Za-z0-9_]*)\$([\s\S]*)\$\1\$\s*;?$/i);
  if (!bodyMatch) {
    return null;
  }

  const header = statement.slice(0, bodyMatch.index).trim();
  const headerMatch = header.match(
    /^create\s+or\s+replace\s+function\s+("?[\w]+"?)\s*\(([\s\S]*?)\)\s*returns\s+([\s\S]+?)\s+language\s+(\w+)([\s\S]*)$/i,
  );
  if (!headerMatch) {
    return null;
  }

  const modifiers = headerMatch[5] ?? "";
  const volatilityMatch = modifiers.match(/\b(immutable|stable|volatile)\b/i);

  return {
    name: unquoteIdent(headerMatch[1]),
    argumentsSql: headerMatch[2].trim(),
    identityArguments: normalizeRoutineArgs(headerMatch[2]),
    returns: headerMatch[3].trim(),
    language: headerMatch[4].toLowerCase(),
    volatility: volatilityMatch ? volatilityMatch[1].toLowerCase() : "volatile",
    bodySql: bodyMatch[2].trim(),
    createStatement: statement.trim(),
  };
}

function parseCreateTrigger(statement) {
  const match = statement.match(
    /^create\s+trigger\s+("?[\w]+"?)\s+(before|after|instead\s+of)\s+([\s\S]+?)\s+on\s+("?[\w]+"?)\s+for\s+each\s+(row|statement)\s+execute\s+(?:function|procedure)\s+("?[\w]+"?)\s*\(([\s\S]*?)\)\s*;?$/i,
  );
  if (!match) {
    return null;
  }

  return {
    name: unquoteIdent(match[1]),
    timing: match[2].replace(/\s+/g, " ").toLowerCase(),
    events: splitTriggerEvents(match[3]),
    tableName: unquoteIdent(match[4]),
    orientation: match[5].toLowerCase(),
    functionName: unquoteIdent(match[6]),
    functionArguments: match[7].trim(),
    createStatement: statement.trim(),
  };
}

function parseCreateIndex(statement) {
  // The indexed table may carry an optional `schema.` prefix; it is stripped so the index
  // associates to the bare table name parsed from CREATE TABLE.
  const match = statement.match(
    /^create\s+(unique\s+)?index\s+(?:if\s+not\s+exists\s+)?("?[\w]+"?)\s+on\s+(?:("?[\w]+"?)\.)?("?[\w]+"?)(?:\s+using\s+(\w+))?\s*\(([\s\S]*?)\)(?:\s+where\s+([\s\S]*?))?\s*;?$/i,
  );
  if (!match) {
    return null;
  }

  const columns = splitTopLevelComma(match[6]).map((item) => {
    const trimmed = item.trim();
    const columnMatch = trimmed.match(/^"?([\w]+)"?(?:\s+(asc|desc))?$/i);
    if (!columnMatch) {
      return trimmed;
    }
    if (!columnMatch[2]) {
      return columnMatch[1];
    }
    return {
      name: columnMatch[1],
      order: columnMatch[2].toLowerCase(),
    };
  });

  return {
    name: unquoteIdent(match[2]),
    tableName: unquoteIdent(match[4]),
    unique: Boolean(match[1]),
    method: match[5]?.toLowerCase(),
    columns,
    where: match[7]?.trim(),
    createStatement: statement.trim(),
  };
}

function normalizeRoutineArgs(value) {
  return value.replace(/\s+/g, " ").trim().toLowerCase();
}

function splitTriggerEvents(value) {
  return value
    .split(/\s+or\s+/i)
    .map((item) => item.trim().toLowerCase())
    .filter(Boolean)
    .sort();
}

function parseCheckConstraint(value) {
  const match = value.match(/^constraint\s+("?[\w]+"?)\s+check\s*\(([\s\S]*)\)$/i);
  if (!match) {
    return null;
  }
  return {
    name: unquoteIdent(match[1]),
    sql: match[2].trim(),
  };
}

function parseColumn(value) {
  const match = value.match(/^("?[\w]+"?)\s+(.+)$/);
  if (!match) {
    return null;
  }

  const name = unquoteIdent(match[1]);
  const rest = match[2].trim();
  const typeMatch = rest.match(/^([a-zA-Z_][\w]*(?:\s*\(\s*\d+\s*\))?)/);
  if (!typeMatch) {
    return null;
  }

  const typeSql = typeMatch[1].replace(/\s+/g, "");
  const maxLengthMatch = typeSql.match(/^varchar\((\d+)\)$/i);
  const sqlType = maxLengthMatch ? "varchar" : typeSql.toLowerCase();
  const defaultSql = extractDefault(rest);
  const column = {
    name,
    kind: kindFromSqlType(sqlType),
    sqlType,
    maxLength: maxLengthMatch ? Number(maxLengthMatch[1]) : undefined,
    primaryKey: /\bprimary\s+key\b/i.test(rest),
    notNull: /\bnot\s+null\b/i.test(rest) || /\bprimary\s+key\b/i.test(rest),
    defaultSql,
    defaultValue: defaultValueFromSql(defaultSql),
    definitionSql: value.replace(/,$/, "").trim(),
  };
  if (column.sqlType === "varchar" && column.maxLength) {
    mergeValidation(column, { maxLength: column.maxLength });
  }

  return Object.fromEntries(Object.entries(column).filter(([, item]) => item !== undefined));
}

function applyCheckValidation(columns, check) {
  const columnByName = new Map(columns.map((column) => [column.name, column]));
  // CHECK clauses can be compound (e.g. `max_warm between 1 and 128 and max_warm >= min_warm`).
  // Split on top-level AND and let each clause try to match a known shape so compound checks
  // still contribute every fact they can, instead of being dropped wholesale.
  for (const clause of splitTopLevelAnd(check.sql)) {
    applyCheckClause(columnByName, clause.trim());
  }
}

function applyCheckClause(columnByName, clauseSql) {
  if (!clauseSql) {
    return;
  }

  // Strip a single leading null guard like `col is null or <inner>` so range checks on nullable
  // columns (e.g. `nats_subject is null or octet_length(nats_subject) <= 256`) still capture
  // their inner constraint without losing nullability semantics.
  const nullGuardMatch = clauseSql.match(/^([\w]+)\s+is\s+null\s+or\s+([\s\S]+)$/i);
  if (nullGuardMatch) {
    applyCheckClause(columnByName, nullGuardMatch[2].trim());
    return;
  }

  const regexMatch = clauseSql.match(/^([\w]+)\s*~\s*'([\s\S]+)'$/i);
  if (regexMatch) {
    // SQL string literals double single-quotes (`''`) to escape an embedded apostrophe; unescape
    // so the captured pattern is the literal regex other languages will compile.
    const pattern = regexMatch[2].replace(/''/g, "'");
    mergeValidation(columnByName.get(regexMatch[1]), { regex: pattern });
    return;
  }

  const maxBytesMatch = clauseSql.match(/^octet_length\(([\w]+)\)\s*<=\s*(\d+)$/i);
  if (maxBytesMatch) {
    mergeValidation(columnByName.get(maxBytesMatch[1]), { maxBytes: Number(maxBytesMatch[2]) });
    return;
  }

  const bytesBetweenMatch = clauseSql.match(
    /^octet_length\(([\w]+)\)\s+between\s+(\d+)\s+and\s+(\d+)$/i,
  );
  if (bytesBetweenMatch) {
    mergeValidation(columnByName.get(bytesBetweenMatch[1]), {
      minBytes: Number(bytesBetweenMatch[2]),
      maxBytes: Number(bytesBetweenMatch[3]),
    });
    return;
  }

  const intBetweenMatch = clauseSql.match(/^([\w]+)\s+between\s+(-?\d+)\s+and\s+(-?\d+)$/i);
  if (intBetweenMatch) {
    mergeValidation(columnByName.get(intBetweenMatch[1]), {
      min: Number(intBetweenMatch[2]),
      max: Number(intBetweenMatch[3]),
    });
    return;
  }

  const intCmpMatch = clauseSql.match(/^([\w]+)\s*(>=|<=|>|<)\s*(-?\d+)$/);
  if (intCmpMatch) {
    const target = columnByName.get(intCmpMatch[1]);
    if (target) {
      const limit = Number(intCmpMatch[3]);
      switch (intCmpMatch[2]) {
        case ">=":
          mergeValidation(target, { min: limit });
          break;
        case ">":
          mergeValidation(target, { min: limit + 1 });
          break;
        case "<=":
          mergeValidation(target, { max: limit });
          break;
        case "<":
          mergeValidation(target, { max: limit - 1 });
          break;
      }
    }
    return;
  }

  const literalMatch = clauseSql.match(/^([\w]+)\s*=\s*'((?:''|[^'])*)'$/i);
  if (literalMatch) {
    mergeValidation(columnByName.get(literalMatch[1]), {
      literal: literalMatch[2].replace(/''/g, "'"),
    });
    return;
  }

  const jsonMatch = clauseSql.match(/^jsonb_typeof\(([\w]+)\)\s*=\s*'(array|object)'$/i);
  if (jsonMatch) {
    const column = columnByName.get(jsonMatch[1]);
    if (column) {
      column.kind = jsonMatch[2] === "array" ? "jsonArray" : "jsonObject";
    }
    return;
  }

  const enumMatch = clauseSql.match(/^([\w]+)\s+in\s+\(([\s\S]+)\)$/i);
  if (enumMatch) {
    const values = splitTopLevelComma(enumMatch[2]).map((item) =>
      item.trim().replace(/^'|'$/g, ""),
    );
    const column = columnByName.get(enumMatch[1]);
    if (column) {
      column.kind = "enum";
      column.enumValues = values;
    }
    return;
  }

  // Cross-column comparisons (e.g. `max_warm >= min_warm`) and other shapes we don't model are
  // intentionally ignored here. They remain enforced by the database; adapters do not need to
  // re-implement them and silently dropping them keeps codegen deterministic.
}

function splitTopLevelAnd(value) {
  // Splits a SQL boolean expression on top-level AND tokens while respecting parentheses,
  // quoted strings, and the embedded AND that lives inside `BETWEEN X AND Y`. The latter is
  // not a logical conjunction and must not become a split point or compound checks like
  // `col between 1 and 64 and col >= other_col` would silently drop the `between` fact.
  // We deliberately do NOT split on OR — null guards like `col is null or octet_length(col)
  // <= N` are handled in `applyCheckClause` so the inner constraint can still be captured.
  const tokens = [];
  let current = "";
  let depth = 0;
  let singleQuoted = false;
  let doubleQuoted = false;
  // betweenAndPending counts how many upcoming AND tokens belong to a still-open BETWEEN. We
  // increment on each `between` keyword and decrement on the matching AND.
  let betweenAndPending = 0;

  const flush = () => {
    const trimmed = current.trim();
    if (trimmed) {
      tokens.push(trimmed);
    }
    current = "";
  };

  for (let index = 0; index < value.length; index += 1) {
    const char = value[index];
    const next = value[index + 1];

    if (char === "'" && !doubleQuoted) {
      current += char;
      if (singleQuoted && next === "'") {
        current += next;
        index += 1;
        continue;
      }
      singleQuoted = !singleQuoted;
      continue;
    }

    if (char === '"' && !singleQuoted) {
      current += char;
      doubleQuoted = !doubleQuoted;
      continue;
    }

    if (singleQuoted || doubleQuoted) {
      current += char;
      continue;
    }

    if (char === "(") {
      depth += 1;
      current += char;
      continue;
    }
    if (char === ")") {
      depth -= 1;
      current += char;
      continue;
    }

    if (depth === 0 && /\s/.test(char)) {
      const remaining = value.slice(index + 1);
      const betweenMatch = remaining.match(/^(between)\s+/i);
      if (betweenMatch) {
        betweenAndPending += 1;
        current += char;
        continue;
      }
      const keywordMatch = remaining.match(/^(and)\s+/i);
      if (keywordMatch) {
        if (betweenAndPending > 0) {
          betweenAndPending -= 1;
          current += char;
          continue;
        }
        flush();
        index += keywordMatch[0].length;
        continue;
      }
    }

    current += char;
  }

  flush();
  return tokens;
}

function applyMetadata(contract) {
  for (const table of contract.tables) {
    const metadata = CODEGEN_METADATA.tables[table.name] ?? {};
    table.description = metadata.description ?? table.description;
    table.names = metadata.names ?? table.names;

    for (const column of table.columns) {
      const columnMetadata = metadata.columns?.[column.name] ?? {};
      if (columnMetadata.generated !== undefined) {
        column.generated = columnMetadata.generated;
      }
      if (columnMetadata.validation) {
        mergeValidation(column, columnMetadata.validation);
      }
    }
  }
}

function splitTopLevelComma(value) {
  const items = [];
  let current = "";
  let depth = 0;
  let singleQuoted = false;
  let doubleQuoted = false;

  for (let index = 0; index < value.length; index += 1) {
    const char = value[index];
    const next = value[index + 1];

    current += char;

    if (char === "'" && !doubleQuoted) {
      if (singleQuoted && next === "'") {
        current += next;
        index += 1;
        continue;
      }
      singleQuoted = !singleQuoted;
      continue;
    }

    if (char === '"' && !singleQuoted) {
      doubleQuoted = !doubleQuoted;
      continue;
    }

    if (singleQuoted || doubleQuoted) {
      continue;
    }

    if (char === "(") {
      depth += 1;
      continue;
    }
    if (char === ")") {
      depth -= 1;
      continue;
    }
    if (char === "," && depth === 0) {
      items.push(current.slice(0, -1));
      current = "";
    }
  }

  const trailing = current.trim();
  if (trailing) {
    items.push(trailing);
  }

  return items;
}

function extractDefault(rest) {
  const lower = rest.toLowerCase();
  const defaultIndex = lower.indexOf(" default ");
  if (defaultIndex === -1) {
    return undefined;
  }

  const start = defaultIndex + " default ".length;
  const candidates = [" not null", " primary key", " constraint"]
    .map((keyword) => lower.indexOf(keyword, start))
    .filter((index) => index !== -1);
  const end = candidates.length > 0 ? Math.min(...candidates) : rest.length;
  return rest.slice(start, end).trim();
}

function defaultValueFromSql(defaultSql) {
  if (!defaultSql) {
    return undefined;
  }
  if (/^'.*'$/.test(defaultSql)) {
    return defaultSql.slice(1, -1).replace(/''/g, "'");
  }
  if (/^-?\d+$/.test(defaultSql)) {
    return Number(defaultSql);
  }
  if (defaultSql === "false") {
    return false;
  }
  if (defaultSql === "true") {
    return true;
  }
  if (defaultSql === "'{}'::jsonb") {
    return {};
  }
  if (defaultSql === "'[]'::jsonb") {
    return [];
  }
  return undefined;
}

function kindFromSqlType(sqlType) {
  switch (sqlType) {
    case "integer":
      return "integer";
    case "bigint":
    case "bigserial":
      return "bigint";
    case "boolean":
      return "boolean";
    case "jsonb":
      return "jsonObject";
    case "timestamptz":
      return "timestamp";
    case "uuid":
      return "uuid";
    default:
      return "string";
  }
}

function mergeValidation(column, validation) {
  if (!column) {
    return;
  }
  column.validation = {
    ...(column.validation ?? {}),
    ...validation,
  };
}

function unquoteIdent(value) {
  return value.replace(/^"|"$/g, "");
}

// ---------------------------------------------------------------------------
// Index helpers shared with the live-drift diff (src/diff.mjs)
// ---------------------------------------------------------------------------

/** Quote a Postgres identifier only when it isn't already a safe bare ident. */
function quoteIdentifier(value) {
  const bare = unquoteIdent(String(value));
  return /^[a-z_][a-z0-9_]*$/.test(bare) ? bare : `"${bare.replace(/"/g, '""')}"`;
}

/** A contract index column is either a bare string or `{ name, order }`. */
function indexColumnName(column) {
  if (!column) {
    return null;
  }
  const name = typeof column === "string" ? column : column.name;
  return name ? unquoteIdent(String(name)) : null;
}

/**
 * Extract the leading column name from a `CREATE INDEX` / `pg_get_indexdef`
 * statement, lowercased and unquoted. Returns null for an unparseable or
 * expression-leading index (so the caller treats it as "not supporting" a
 * plain foreign-key column rather than falsely claiming coverage).
 */
export function parseIndexLeadingColumn(definition) {
  if (!definition) {
    return null;
  }
  let def = String(definition).trim();
  if (def.endsWith(";")) {
    def = def.slice(0, -1).trim();
  }
  def = def.replace(/\bconcurrently\b\s*/gi, "").replace(/\bif\s+not\s+exists\b\s*/gi, "");
  // CREATE [UNIQUE] INDEX name ON [schema.]table [USING method] (
  const head = def.match(
    /^create\s+(?:unique\s+)?index\s+"?[\w]+"?\s+on\s+(?:"?[\w]+"?\.)?"?[\w]+"?\s*(?:using\s+\w+\s*)?\(/i,
  );
  if (!head) {
    return null;
  }
  // Walk balanced parens from the opening `(` to capture the column list.
  const open = def.indexOf("(", head[0].length - 1);
  let depth = 0;
  let end = -1;
  for (let i = open; i < def.length; i += 1) {
    if (def[i] === "(") depth += 1;
    else if (def[i] === ")") {
      depth -= 1;
      if (depth === 0) {
        end = i;
        break;
      }
    }
  }
  if (end === -1) {
    return null;
  }
  const inner = def.slice(open + 1, end);
  const first = splitTopLevelComma(inner)[0]?.trim();
  if (!first) {
    return null;
  }
  // Only a plain `col` / `"col"` / `col asc|desc` leads to a usable column.
  const plain = first.match(/^"?([a-z_][\w$]*)"?(?:\s+(?:asc|desc))?(?:\s+nulls\s+(?:first|last))?$/i);
  return plain ? plain[1].toLowerCase() : null;
}

/** Build the derived FK-support index name, capped to Postgres's 63-char limit. */
export function foreignKeyIndexName(tableName, columnName) {
  const base = `${unquoteIdent(tableName)}_${unquoteIdent(columnName)}_fk_idx`;
  return base.length <= 63 ? base : base.slice(0, 63);
}

/**
 * Given the parsed contract `tables` and a `liveSupport` set (keys
 * `"tableName column"`, lowercased, of FK columns already led by a LIVE
 * index), return the foreign keys that lack ANY supporting index — declared
 * (a contract index whose leading column is the FK column), implicit (the FK
 * column is a primary-key column), or live — together with an idempotent
 * `create index if not exists` statement. Pure + deterministic; the live set
 * is empty when deriving from the contract alone (fresh DB / unit tests).
 */
export function foreignKeyIndexRecommendations(tables, liveSupport = new Set()) {
  const recommendations = [];
  for (const table of tables ?? []) {
    const schema = table.schema ?? "public";
    const declaredLeading = new Set();
    for (const index of table.indexes ?? []) {
      const lead = indexColumnName(index.columns?.[0]);
      if (lead) {
        declaredLeading.add(lead.toLowerCase());
      }
    }
    const pkColumns = new Set(
      (table.columns ?? []).filter((c) => c.primaryKey).map((c) => c.name.toLowerCase()),
    );
    const seen = new Set();
    for (const fk of table.foreignKeys ?? []) {
      const column = unquoteIdent(fk.column ?? "").toLowerCase();
      if (!column || seen.has(column)) {
        continue;
      }
      seen.add(column);
      const liveKey = `${table.name.toLowerCase()} ${column}`;
      if (declaredLeading.has(column) || pkColumns.has(column) || liveSupport.has(liveKey)) {
        continue;
      }
      const indexName = foreignKeyIndexName(table.name, fk.column);
      const qualified =
        schema === "public"
          ? quoteIdentifier(table.name)
          : `${quoteIdentifier(schema)}.${quoteIdentifier(table.name)}`;
      recommendations.push({
        schema,
        table: table.name,
        column: fk.column,
        constraint: fk.name ?? null,
        indexName,
        statement: `create index if not exists ${quoteIdentifier(indexName)} on ${qualified} (${quoteIdentifier(fk.column)});`,
      });
    }
  }
  return recommendations;
}
