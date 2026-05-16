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
        file_name: {
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
  const tableByName = new Map();

  for (const statement of statements) {
    const table = parseCreateTable(statement);
    if (!table) {
      continue;
    }

    tables.push(table);
    tableByName.set(table.name, table);
  }

  for (const statement of statements) {
    const index = parseCreateIndex(statement);
    if (!index) {
      continue;
    }

    const table = tableByName.get(index.tableName);
    if (table) {
      table.indexes.push(index);
    }
  }

  return {
    contractVersion: "sql-source",
    dialect: "postgresql",
    description: "Generated from schema/schema.sql.",
    tables,
  };
}

export function splitSqlStatements(sourceSql) {
  const statements = [];
  let current = "";
  let singleQuoted = false;
  let doubleQuoted = false;
  let lineComment = false;

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
  const match = statement.match(
    /^create\s+table\s+(?:if\s+not\s+exists\s+)?("?[\w]+"?)\s*\(([\s\S]*)\)\s*;?$/i,
  );
  if (!match) {
    return null;
  }

  const tableName = unquoteIdent(match[1]);
  const body = match[2].trim();
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
    description: "",
    names: {},
    columns,
    checks,
    indexes: [],
    createStatement: statement.trim(),
  };
}

function parseCreateIndex(statement) {
  const match = statement.match(
    /^create\s+(unique\s+)?index\s+(?:if\s+not\s+exists\s+)?("?[\w]+"?)\s+on\s+("?[\w]+"?)(?:\s+using\s+(\w+))?\s*\(([\s\S]*?)\)(?:\s+where\s+([\s\S]*?))?\s*;?$/i,
  );
  if (!match) {
    return null;
  }

  const columns = splitTopLevelComma(match[5]).map((item) => {
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
    tableName: unquoteIdent(match[3]),
    unique: Boolean(match[1]),
    method: match[4]?.toLowerCase(),
    columns,
    where: match[6]?.trim(),
    createStatement: statement.trim(),
  };
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
  const regexMatch = check.sql.match(/^([\w]+)\s*~\s*'([^']+)'$/i);
  if (regexMatch) {
    mergeValidation(columnByName.get(regexMatch[1]), { regex: regexMatch[2] });
  }

  const maxBytesMatch = check.sql.match(/^octet_length\(([\w]+)\)\s*<=\s*(\d+)$/i);
  if (maxBytesMatch) {
    mergeValidation(columnByName.get(maxBytesMatch[1]), { maxBytes: Number(maxBytesMatch[2]) });
  }

  const literalMatch = check.sql.match(/^([\w]+)\s*=\s*'((?:''|[^'])*)'$/i);
  if (literalMatch) {
    mergeValidation(columnByName.get(literalMatch[1]), { literal: literalMatch[2].replace(/''/g, "'") });
  }

  const jsonMatch = check.sql.match(/^jsonb_typeof\(([\w]+)\)\s*=\s*'(array|object)'$/i);
  if (jsonMatch) {
    const column = columnByName.get(jsonMatch[1]);
    if (column) {
      column.kind = jsonMatch[2] === "array" ? "jsonArray" : "jsonObject";
    }
  }

  const enumMatch = check.sql.match(/^([\w]+)\s+in\s+\(([\s\S]+)\)$/i);
  if (enumMatch) {
    const values = splitTopLevelComma(enumMatch[2]).map((item) => item.trim().replace(/^'|'$/g, ""));
    const column = columnByName.get(enumMatch[1]);
    if (column) {
      column.kind = "enum";
      column.enumValues = values;
    }
  }
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
  if (/^\d+$/.test(defaultSql)) {
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
