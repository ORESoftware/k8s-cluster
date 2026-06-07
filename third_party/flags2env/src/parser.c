#include "parser.h"

#include <ctype.h>
#include <errno.h>
#include <limits.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#if defined(__APPLE__)
#include <sys/ioctl.h>
#include <sys/sysctl.h>
#include <unistd.h>
#elif defined(__unix__)
#include <sys/ioctl.h>
#include <unistd.h>
#elif defined(_WIN32)
#include <shellapi.h>
#include <windows.h>
#endif

#ifndef PATH_MAX
#define PATH_MAX 4096
#endif

#define F2E_MAX_FLAGS 256
#define F2E_MAX_ALIASES 24
#define F2E_MAX_NAME 96
#define F2E_MAX_ENV 128
#define F2E_MAX_VALUE 1024
#define F2E_MAX_LINE 4096
#define F2E_MAX_META_PAIRS 3
#define F2E_MAX_PAIRS (F2E_MAX_FLAGS + F2E_MAX_META_PAIRS)
#define F2E_MAX_ENV_FILE_KEYS 512

#define F2E_HELP_COL_OPTIONS (1u << 0)
#define F2E_HELP_COL_ENV (1u << 1)
#define F2E_HELP_COL_TYPE (1u << 2)
#define F2E_HELP_COL_DEFAULT (1u << 3)
#define F2E_HELP_COL_DESCRIPTION (1u << 4)
#define F2E_HELP_COL_DEFAULTS \
  (F2E_HELP_COL_OPTIONS | F2E_HELP_COL_ENV | F2E_HELP_COL_TYPE | F2E_HELP_COL_DEFAULT | F2E_HELP_COL_DESCRIPTION)

typedef enum {
  F2E_TYPE_STRING = 0,
  F2E_TYPE_BOOL = 1,
  F2E_TYPE_INT = 2,
  F2E_TYPE_JSON = 3
} F2EValueType;

typedef struct {
  char name[F2E_MAX_NAME];
  char env[F2E_MAX_ENV];
  char aliases[F2E_MAX_ALIASES][F2E_MAX_NAME];
  size_t alias_count;
  char true_aliases[F2E_MAX_ALIASES][F2E_MAX_NAME];
  size_t true_alias_count;
  char false_aliases[F2E_MAX_ALIASES][F2E_MAX_NAME];
  size_t false_alias_count;
  char short_name;
  F2EValueType type;
  int invalid_type;
  char type_value[F2E_MAX_VALUE];
  int has_default;
  char default_value[F2E_MAX_VALUE];
  char help[F2E_MAX_VALUE];
} F2EFlag;

typedef struct {
  F2EFlag flags[F2E_MAX_FLAGS];
  size_t flag_count;
  int allow_separated_values;
  int stop_at_first_positional;
  char positionals_env[F2E_MAX_ENV];
  char unknown_options_env[F2E_MAX_ENV];
  char errors_env[F2E_MAX_ENV];
  int allow_unknown;
  char env_audit_ignored_keys[F2E_MAX_ENV_FILE_KEYS][F2E_MAX_ENV];
  size_t env_audit_ignored_count;
  int invalid_env_audit_ignore;
  char help_url[F2E_MAX_VALUE];
  unsigned help_columns;
  int help_columns_configured;
  unsigned help_exclude_columns;
  int invalid_help_columns;
  int invalid_help_exclude_columns;
} F2EConfig;

typedef struct {
  char key[F2E_MAX_ENV];
  char value[F2E_MAX_VALUE];
  int set;
} F2EPair;

typedef struct {
  char *data;
  size_t len;
  size_t cap;
} F2EBuffer;

typedef struct {
  F2EBuffer errors;
  F2EBuffer warnings;
  size_t error_count;
  size_t warning_count;
  int failed;
} F2EAudit;

typedef enum {
  F2E_SECTION_NONE = 0,
  F2E_SECTION_PARSE = 1,
  F2E_SECTION_FLAG = 2,
  F2E_SECTION_HELP = 3,
  F2E_SECTION_ENV_AUDIT = 4
} F2EConfigSection;

typedef struct {
  F2EBuffer buffer;
  size_t count;
  int initialized;
  int failed;
} F2EJsonList;

static size_t f2e_strlcpy(char *dst, const char *src, size_t dst_size) {
  size_t src_len = src ? strlen(src) : 0;
  if (dst_size > 0) {
    size_t copy_len = src_len >= dst_size ? dst_size - 1 : src_len;
    if (copy_len > 0 && src) {
      memcpy(dst, src, copy_len);
    }
    dst[copy_len] = '\0';
  }
  return src_len;
}

static char *f2e_strdup(const char *value) {
  size_t len = value ? strlen(value) : 0;
  char *copy = (char *)malloc(len + 1);
  if (!copy) {
    return NULL;
  }
  if (len > 0 && value) {
    memcpy(copy, value, len);
  }
  copy[len] = '\0';
  return copy;
}

static int f2e_streq(const char *a, const char *b) {
  return strcmp(a, b) == 0;
}

static int f2e_env_name_is_valid(const char *value) {
  if (!value || value[0] == '\0') {
    return 0;
  }
  if (!(isalpha((unsigned char)value[0]) || value[0] == '_')) {
    return 0;
  }
  for (const char *cursor = value + 1; *cursor; cursor++) {
    if (!(isalnum((unsigned char)*cursor) || *cursor == '_')) {
      return 0;
    }
  }
  return 1;
}

static int f2e_option_name_is_valid(const char *value) {
  if (!value || value[0] == '\0') {
    return 0;
  }
  for (const unsigned char *cursor = (const unsigned char *)value; *cursor; cursor++) {
    if (!(isalnum(*cursor) || *cursor == '-' || *cursor == '_' || *cursor == '.')) {
      return 0;
    }
  }
  return value[0] != '-';
}

static int f2e_shell_word_chars_are_valid(const char *value, size_t len) {
  if (!value || len == 0) {
    return 0;
  }
  for (size_t i = 0; i < len; i++) {
    unsigned char ch = (unsigned char)value[i];
    if (!(isalnum(ch) || ch == '-' || ch == '_' || ch == '.')) {
      return 0;
    }
  }
  return 1;
}

static int f2e_shell_word_is_valid(const char *value) {
  return value ? f2e_shell_word_chars_are_valid(value, strlen(value)) : 0;
}

static int f2e_path_basename_copy(const char *value, char *out, size_t out_size) {
  int used_default = !value || value[0] == '\0';
  const char *path = used_default ? "flags2env" : value;
  size_t len = strlen(path);
  while (len > 0 && (path[len - 1] == '/' || path[len - 1] == '\\')) {
    len--;
  }
  if (len == 0) {
    if (!used_default) {
      return 0;
    }
    path = "flags2env";
    len = strlen(path);
  }

  size_t start = 0;
  for (size_t i = len; i > 0; i--) {
    if (path[i - 1] == '/' || path[i - 1] == '\\') {
      start = i;
      break;
    }
  }

  size_t base_len = len - start;
  if (!out || out_size == 0 || base_len == 0 || base_len >= out_size ||
      !f2e_shell_word_chars_are_valid(path + start, base_len)) {
    return 0;
  }
  memcpy(out, path + start, base_len);
  out[base_len] = '\0';
  return 1;
}

static char *f2e_empty_json_object(void);
static const char *f2e_audit_flag_name(const F2EFlag *flag);

static char *f2e_trim_left(char *value) {
  while (*value && isspace((unsigned char)*value)) {
    value++;
  }
  return value;
}

static void f2e_trim_right(char *value) {
  size_t len = strlen(value);
  while (len > 0 && isspace((unsigned char)value[len - 1])) {
    value[len - 1] = '\0';
    len--;
  }
}

static char *f2e_trim(char *value) {
  char *left = f2e_trim_left(value);
  f2e_trim_right(left);
  return left;
}

static void f2e_strip_comment(char *line) {
  int in_quote = 0;
  int escaped = 0;
  for (char *cursor = line; *cursor; cursor++) {
    if (escaped) {
      escaped = 0;
      continue;
    }
    if (*cursor == '\\' && in_quote) {
      escaped = 1;
      continue;
    }
    if (*cursor == '"') {
      in_quote = !in_quote;
      continue;
    }
    if (*cursor == '#' && !in_quote) {
      *cursor = '\0';
      return;
    }
  }
}

static int f2e_parse_quoted_string(const char *input, char *out, size_t out_size) {
  const char *cursor = f2e_trim_left((char *)input);
  size_t len = 0;
  if (*cursor != '"') {
    return 0;
  }
  cursor++;
  while (*cursor && *cursor != '"') {
    char ch = *cursor++;
    if (ch == '\\' && *cursor) {
      char escaped = *cursor++;
      switch (escaped) {
        case 'n':
          ch = '\n';
          break;
        case 'r':
          ch = '\r';
          break;
        case 't':
          ch = '\t';
          break;
        case '"':
        case '\\':
        case '/':
          ch = escaped;
          break;
        default:
          ch = escaped;
          break;
      }
    }
    if (len + 1 < out_size) {
      out[len++] = ch;
    }
  }
  if (*cursor != '"') {
    return 0;
  }
  if (out_size > 0) {
    out[len] = '\0';
  }
  return 1;
}

static int f2e_parse_bare_value(const char *input, char *out, size_t out_size) {
  char tmp[F2E_MAX_VALUE];
  f2e_strlcpy(tmp, input, sizeof(tmp));
  char *trimmed = f2e_trim(tmp);
  if (*trimmed == '"') {
    return f2e_parse_quoted_string(trimmed, out, out_size);
  }
  f2e_strlcpy(out, trimmed, out_size);
  return out[0] != '\0';
}

static int f2e_add_alias_to_list(char aliases[][F2E_MAX_NAME], size_t *alias_count, const char *alias) {
  if (!alias || alias[0] == '\0') {
    return 0;
  }
  for (size_t i = 0; i < *alias_count; i++) {
    if (f2e_streq(aliases[i], alias)) {
      return 1;
    }
  }
  if (*alias_count >= F2E_MAX_ALIASES) {
    return 0;
  }
  f2e_strlcpy(aliases[*alias_count], alias, F2E_MAX_NAME);
  (*alias_count)++;
  return 1;
}

static int f2e_add_alias(F2EFlag *flag, const char *alias) {
  return f2e_add_alias_to_list(flag->aliases, &flag->alias_count, alias);
}

static F2EFlag *f2e_add_flag(F2EConfig *config, const char *name) {
  if (config->flag_count >= F2E_MAX_FLAGS) {
    return NULL;
  }
  F2EFlag *flag = &config->flags[config->flag_count++];
  memset(flag, 0, sizeof(*flag));
  flag->type = F2E_TYPE_STRING;
  f2e_strlcpy(flag->name, name, sizeof(flag->name));
  f2e_add_alias(flag, name);
  return flag;
}

static F2EFlag *f2e_find_flag_by_alias(F2EConfig *config, const char *alias) {
  for (size_t i = 0; i < config->flag_count; i++) {
    F2EFlag *flag = &config->flags[i];
    for (size_t j = 0; j < flag->alias_count; j++) {
      if (f2e_streq(flag->aliases[j], alias)) {
        return flag;
      }
    }
  }
  return NULL;
}

static const F2EFlag *f2e_find_flag_by_alias_const(const F2EConfig *config, const char *alias) {
  for (size_t i = 0; i < config->flag_count; i++) {
    const F2EFlag *flag = &config->flags[i];
    for (size_t j = 0; j < flag->alias_count; j++) {
      if (f2e_streq(flag->aliases[j], alias)) {
        return flag;
      }
    }
  }
  return NULL;
}

static F2EFlag *f2e_find_flag_by_short(F2EConfig *config, char short_name) {
  for (size_t i = 0; i < config->flag_count; i++) {
    if (config->flags[i].short_name == short_name) {
      return &config->flags[i];
    }
  }
  return NULL;
}

static int f2e_parse_alias_list(char aliases[][F2E_MAX_NAME], size_t *alias_count, const char *value) {
  const char *cursor = f2e_trim_left((char *)value);
  if (*cursor != '[') {
    return 0;
  }
  cursor++;
  while (*cursor) {
    cursor = f2e_trim_left((char *)cursor);
    if (*cursor == ']') {
      return 1;
    }
    char alias[F2E_MAX_NAME];
    if (!f2e_parse_quoted_string(cursor, alias, sizeof(alias))) {
      return 0;
    }
    f2e_add_alias_to_list(aliases, alias_count, alias);
    cursor++;
    int escaped = 0;
    while (*cursor) {
      if (escaped) {
        escaped = 0;
      } else if (*cursor == '\\') {
        escaped = 1;
      } else if (*cursor == '"') {
        cursor++;
        break;
      }
      cursor++;
    }
    cursor = f2e_trim_left((char *)cursor);
    if (*cursor == ',') {
      cursor++;
    }
  }
  return 0;
}

static int f2e_parse_aliases(F2EFlag *flag, const char *value) {
  return f2e_parse_alias_list(flag->aliases, &flag->alias_count, value);
}

static int f2e_add_env_key_to_list(char keys[][F2E_MAX_ENV], size_t *key_count, const char *key) {
  if (!keys || !key_count || !key) {
    return 0;
  }
  for (size_t i = 0; i < *key_count; i++) {
    if (f2e_streq(keys[i], key)) {
      return 1;
    }
  }
  if (*key_count >= F2E_MAX_ENV_FILE_KEYS) {
    return 0;
  }
  f2e_strlcpy(keys[*key_count], key, F2E_MAX_ENV);
  (*key_count)++;
  return 1;
}

static int f2e_parse_env_key_list(char keys[][F2E_MAX_ENV], size_t *key_count, const char *value) {
  size_t original_count = key_count ? *key_count : 0;
  const char *cursor = f2e_trim_left((char *)value);
  if (*cursor != '[') {
    return 0;
  }
  cursor++;
  while (*cursor) {
    cursor = f2e_trim_left((char *)cursor);
    if (*cursor == ']') {
      return 1;
    }
    char key[F2E_MAX_ENV];
    if (!f2e_parse_quoted_string(cursor, key, sizeof(key))) {
      if (key_count) {
        *key_count = original_count;
      }
      return 0;
    }
    if (!f2e_add_env_key_to_list(keys, key_count, key)) {
      if (key_count) {
        *key_count = original_count;
      }
      return 0;
    }
    cursor++;
    int escaped = 0;
    while (*cursor) {
      if (escaped) {
        escaped = 0;
      } else if (*cursor == '\\') {
        escaped = 1;
      } else if (*cursor == '"') {
        cursor++;
        break;
      }
      cursor++;
    }
    cursor = f2e_trim_left((char *)cursor);
    if (*cursor == ',') {
      cursor++;
    }
  }
  if (key_count) {
    *key_count = original_count;
  }
  return 0;
}

static unsigned f2e_help_column_mask_for_name(const char *name) {
  if (f2e_streq(name, "options") || f2e_streq(name, "option") ||
      f2e_streq(name, "flags") || f2e_streq(name, "names")) {
    return F2E_HELP_COL_OPTIONS;
  }
  if (f2e_streq(name, "env") || f2e_streq(name, "environment")) {
    return F2E_HELP_COL_ENV;
  }
  if (f2e_streq(name, "type")) {
    return F2E_HELP_COL_TYPE;
  }
  if (f2e_streq(name, "default") || f2e_streq(name, "defaults")) {
    return F2E_HELP_COL_DEFAULT;
  }
  if (f2e_streq(name, "description") || f2e_streq(name, "help")) {
    return F2E_HELP_COL_DESCRIPTION;
  }
  return 0;
}

static int f2e_parse_help_column_list(const char *value, unsigned *mask_out) {
  const char *cursor = f2e_trim_left((char *)value);
  unsigned mask = 0;
  if (*cursor != '[') {
    return 0;
  }
  cursor++;
  while (*cursor) {
    cursor = f2e_trim_left((char *)cursor);
    if (*cursor == ']') {
      *mask_out = mask;
      return 1;
    }
    char column[F2E_MAX_NAME];
    if (!f2e_parse_quoted_string(cursor, column, sizeof(column))) {
      return 0;
    }
    unsigned column_mask = f2e_help_column_mask_for_name(column);
    if (column_mask == 0) {
      return 0;
    }
    mask |= column_mask;
    cursor++;
    int escaped = 0;
    while (*cursor) {
      if (escaped) {
        escaped = 0;
      } else if (*cursor == '\\') {
        escaped = 1;
      } else if (*cursor == '"') {
        cursor++;
        break;
      }
      cursor++;
    }
    cursor = f2e_trim_left((char *)cursor);
    if (*cursor == ',') {
      cursor++;
    }
  }
  return 0;
}

static int f2e_parse_true_aliases(F2EFlag *flag, const char *value) {
  return f2e_parse_alias_list(flag->true_aliases, &flag->true_alias_count, value);
}

static int f2e_parse_false_aliases(F2EFlag *flag, const char *value) {
  return f2e_parse_alias_list(flag->false_aliases, &flag->false_alias_count, value);
}

static int f2e_parse_type(const char *value, F2EValueType *type) {
  char parsed[F2E_MAX_VALUE];
  if (!f2e_parse_bare_value(value, parsed, sizeof(parsed))) {
    return 0;
  }
  if (f2e_streq(parsed, "bool") || f2e_streq(parsed, "boolean") || f2e_streq(parsed, "flag")) {
    *type = F2E_TYPE_BOOL;
    return 1;
  }
  if (f2e_streq(parsed, "string") || f2e_streq(parsed, "value")) {
    *type = F2E_TYPE_STRING;
    return 1;
  }
  if (f2e_streq(parsed, "int") || f2e_streq(parsed, "integer")) {
    *type = F2E_TYPE_INT;
    return 1;
  }
  if (f2e_streq(parsed, "json")) {
    *type = F2E_TYPE_JSON;
    return 1;
  }
  return 0;
}

static int f2e_parse_config_bool(const char *value, int *out) {
  char parsed[F2E_MAX_VALUE];
  if (!f2e_parse_bare_value(value, parsed, sizeof(parsed))) {
    return 0;
  }
  if (f2e_streq(parsed, "true") || f2e_streq(parsed, "1") || f2e_streq(parsed, "yes") || f2e_streq(parsed, "on")) {
    *out = 1;
    return 1;
  }
  if (f2e_streq(parsed, "false") || f2e_streq(parsed, "0") || f2e_streq(parsed, "no") || f2e_streq(parsed, "off")) {
    *out = 0;
    return 1;
  }
  return 0;
}

static void f2e_json_skip_ws(const char **cursor) {
  while (**cursor == ' ' || **cursor == '\n' || **cursor == '\r' || **cursor == '\t') {
    (*cursor)++;
  }
}

static int f2e_json_is_hex(char ch) {
  return (ch >= '0' && ch <= '9') ||
         (ch >= 'a' && ch <= 'f') ||
         (ch >= 'A' && ch <= 'F');
}

static int f2e_json_parse_value(const char **cursor, int depth);

static int f2e_json_parse_string_value(const char **cursor) {
  if (**cursor != '"') {
    return 0;
  }
  (*cursor)++;
  while (**cursor) {
    unsigned char ch = (unsigned char)**cursor;
    if (ch == '"') {
      (*cursor)++;
      return 1;
    }
    if (ch < 0x20) {
      return 0;
    }
    if (ch == '\\') {
      (*cursor)++;
      switch (**cursor) {
        case '"':
        case '\\':
        case '/':
        case 'b':
        case 'f':
        case 'n':
        case 'r':
        case 't':
          (*cursor)++;
          break;
        case 'u':
          (*cursor)++;
          for (int i = 0; i < 4; i++) {
            if (!f2e_json_is_hex((*cursor)[i])) {
              return 0;
            }
          }
          *cursor += 4;
          break;
        default:
          return 0;
      }
      continue;
    }
    (*cursor)++;
  }
  return 0;
}

static int f2e_json_parse_number(const char **cursor) {
  const char *start = *cursor;
  if (**cursor == '-') {
    (*cursor)++;
  }
  if (**cursor == '0') {
    (*cursor)++;
  } else if (**cursor >= '1' && **cursor <= '9') {
    while (**cursor >= '0' && **cursor <= '9') {
      (*cursor)++;
    }
  } else {
    return 0;
  }
  if (**cursor == '.') {
    (*cursor)++;
    if (!(**cursor >= '0' && **cursor <= '9')) {
      return 0;
    }
    while (**cursor >= '0' && **cursor <= '9') {
      (*cursor)++;
    }
  }
  if (**cursor == 'e' || **cursor == 'E') {
    (*cursor)++;
    if (**cursor == '+' || **cursor == '-') {
      (*cursor)++;
    }
    if (!(**cursor >= '0' && **cursor <= '9')) {
      return 0;
    }
    while (**cursor >= '0' && **cursor <= '9') {
      (*cursor)++;
    }
  }
  return *cursor > start;
}

static int f2e_json_parse_literal(const char **cursor, const char *literal) {
  size_t len = strlen(literal);
  if (strncmp(*cursor, literal, len) != 0) {
    return 0;
  }
  *cursor += len;
  return 1;
}

static int f2e_json_parse_array(const char **cursor, int depth) {
  if (**cursor != '[') {
    return 0;
  }
  (*cursor)++;
  f2e_json_skip_ws(cursor);
  if (**cursor == ']') {
    (*cursor)++;
    return 1;
  }
  while (**cursor) {
    if (!f2e_json_parse_value(cursor, depth + 1)) {
      return 0;
    }
    f2e_json_skip_ws(cursor);
    if (**cursor == ',') {
      (*cursor)++;
      f2e_json_skip_ws(cursor);
      if (**cursor == ']') {
        return 0;
      }
      continue;
    }
    if (**cursor == ']') {
      (*cursor)++;
      return 1;
    }
    return 0;
  }
  return 0;
}

static int f2e_json_parse_object(const char **cursor, int depth) {
  if (**cursor != '{') {
    return 0;
  }
  (*cursor)++;
  f2e_json_skip_ws(cursor);
  if (**cursor == '}') {
    (*cursor)++;
    return 1;
  }
  while (**cursor) {
    if (!f2e_json_parse_string_value(cursor)) {
      return 0;
    }
    f2e_json_skip_ws(cursor);
    if (**cursor != ':') {
      return 0;
    }
    (*cursor)++;
    if (!f2e_json_parse_value(cursor, depth + 1)) {
      return 0;
    }
    f2e_json_skip_ws(cursor);
    if (**cursor == ',') {
      (*cursor)++;
      f2e_json_skip_ws(cursor);
      if (**cursor == '}') {
        return 0;
      }
      continue;
    }
    if (**cursor == '}') {
      (*cursor)++;
      return 1;
    }
    return 0;
  }
  return 0;
}

static int f2e_json_parse_value(const char **cursor, int depth) {
  if (depth > 64) {
    return 0;
  }
  f2e_json_skip_ws(cursor);
  switch (**cursor) {
    case '"':
      return f2e_json_parse_string_value(cursor);
    case '{':
      return f2e_json_parse_object(cursor, depth);
    case '[':
      return f2e_json_parse_array(cursor, depth);
    case 't':
      return f2e_json_parse_literal(cursor, "true");
    case 'f':
      return f2e_json_parse_literal(cursor, "false");
    case 'n':
      return f2e_json_parse_literal(cursor, "null");
    default:
      if (**cursor == '-' || (**cursor >= '0' && **cursor <= '9')) {
        return f2e_json_parse_number(cursor);
      }
      return 0;
  }
}

static int f2e_json_value_is_valid(const char *value) {
  if (!value) {
    return 0;
  }
  const char *cursor = value;
  if (!f2e_json_parse_value(&cursor, 0)) {
    return 0;
  }
  f2e_json_skip_ws(&cursor);
  return *cursor == '\0';
}

static int f2e_load_config(const char *config_path, F2EConfig *config) {
  memset(config, 0, sizeof(*config));
  config->allow_separated_values = 1;
  config->help_columns = F2E_HELP_COL_DEFAULTS;

  FILE *file = fopen(config_path, "r");
  if (!file) {
    return 0;
  }

  F2EFlag *current = NULL;
  F2EConfigSection section = F2E_SECTION_NONE;
  char line[F2E_MAX_LINE];
  while (fgets(line, sizeof(line), file)) {
    f2e_strip_comment(line);
    char *trimmed = f2e_trim(line);
    if (trimmed[0] == '\0') {
      continue;
    }

    if (trimmed[0] == '[') {
      char *end = strchr(trimmed, ']');
      if (!end) {
        current = NULL;
        continue;
      }
      *end = '\0';
      char *table = f2e_trim(trimmed + 1);
      const char prefix[] = "flags.";
      if (strncmp(table, prefix, sizeof(prefix) - 1) == 0) {
        char *name = f2e_trim(table + sizeof(prefix) - 1);
        current = f2e_add_flag(config, name);
        section = F2E_SECTION_FLAG;
      } else if (f2e_streq(table, "parse") || f2e_streq(table, "parser")) {
        current = NULL;
        section = F2E_SECTION_PARSE;
      } else if (f2e_streq(table, "help") || f2e_streq(table, "help_menu")) {
        current = NULL;
        section = F2E_SECTION_HELP;
      } else if (f2e_streq(table, "env") ||
                 f2e_streq(table, "env_audit") ||
                 f2e_streq(table, "audit_env") ||
                 f2e_streq(table, "audit.env") ||
                 f2e_streq(table, "dotenv")) {
        current = NULL;
        section = F2E_SECTION_ENV_AUDIT;
      } else {
        current = NULL;
        section = F2E_SECTION_NONE;
      }
      continue;
    }

    char *eq = strchr(trimmed, '=');
    if (!eq) {
      continue;
    }
    *eq = '\0';
    char *key = f2e_trim(trimmed);
    char *value = f2e_trim(eq + 1);

    if (section == F2E_SECTION_PARSE) {
      if (f2e_streq(key, "allow_separated_values") || f2e_streq(key, "allow_space_values")) {
        int parsed = 0;
        if (f2e_parse_config_bool(value, &parsed)) {
          config->allow_separated_values = parsed;
        }
      } else if (f2e_streq(key, "require_equals")) {
        int parsed = 0;
        if (f2e_parse_config_bool(value, &parsed)) {
          config->allow_separated_values = !parsed;
        }
      } else if (f2e_streq(key, "stop_at_first_positional")) {
        int parsed = 0;
        if (f2e_parse_config_bool(value, &parsed)) {
          config->stop_at_first_positional = parsed;
        }
      } else if (f2e_streq(key, "allow_unknown") ||
                 f2e_streq(key, "allow_hidden") ||
                 f2e_streq(key, "allow_unrecognized") ||
                 f2e_streq(key, "allow_unknown_options")) {
        int parsed = 0;
        if (f2e_parse_config_bool(value, &parsed)) {
          config->allow_unknown = parsed;
        }
      } else if (f2e_streq(key, "positionals_env") || f2e_streq(key, "extras_env")) {
        char parsed[F2E_MAX_ENV];
        if (f2e_parse_bare_value(value, parsed, sizeof(parsed))) {
          f2e_strlcpy(config->positionals_env, parsed, sizeof(config->positionals_env));
        }
      } else if (f2e_streq(key, "unknown_options_env")) {
        char parsed[F2E_MAX_ENV];
        if (f2e_parse_bare_value(value, parsed, sizeof(parsed))) {
          f2e_strlcpy(config->unknown_options_env, parsed, sizeof(config->unknown_options_env));
        }
      } else if (f2e_streq(key, "errors_env") || f2e_streq(key, "parse_errors_env")) {
        char parsed[F2E_MAX_ENV];
        if (f2e_parse_bare_value(value, parsed, sizeof(parsed))) {
          f2e_strlcpy(config->errors_env, parsed, sizeof(config->errors_env));
        }
      } else if (f2e_streq(key, "help_url") || f2e_streq(key, "url")) {
        char parsed[F2E_MAX_VALUE];
        if (f2e_parse_bare_value(value, parsed, sizeof(parsed))) {
          f2e_strlcpy(config->help_url, parsed, sizeof(config->help_url));
        }
      } else if (f2e_streq(key, "env_audit_ignore") ||
                 f2e_streq(key, "env_audit_ignore_keys") ||
                 f2e_streq(key, "ignore_env") ||
                 f2e_streq(key, "ignore_envs") ||
                 f2e_streq(key, "ignored_envs") ||
                 f2e_streq(key, "ignored_env")) {
        if (!f2e_parse_env_key_list(config->env_audit_ignored_keys,
                                    &config->env_audit_ignored_count,
                                    value)) {
          config->invalid_env_audit_ignore = 1;
        }
      }
      continue;
    }

    if (section == F2E_SECTION_HELP) {
      if (f2e_streq(key, "url") || f2e_streq(key, "help_url")) {
        char parsed[F2E_MAX_VALUE];
        if (f2e_parse_bare_value(value, parsed, sizeof(parsed))) {
          f2e_strlcpy(config->help_url, parsed, sizeof(config->help_url));
        }
      } else if (f2e_streq(key, "columns") ||
                 f2e_streq(key, "include") ||
                 f2e_streq(key, "include_columns") ||
                 f2e_streq(key, "table_columns")) {
        unsigned parsed = 0;
        if (f2e_parse_help_column_list(value, &parsed)) {
          config->help_columns = parsed;
          config->help_columns_configured = 1;
        } else {
          config->invalid_help_columns = 1;
        }
      } else if (f2e_streq(key, "exclude") ||
                 f2e_streq(key, "exclude_columns") ||
                 f2e_streq(key, "table_exclude")) {
        unsigned parsed = 0;
        if (f2e_parse_help_column_list(value, &parsed)) {
          config->help_exclude_columns |= parsed;
        } else {
          config->invalid_help_exclude_columns = 1;
        }
      }
      continue;
    }

    if (section == F2E_SECTION_ENV_AUDIT) {
      if (f2e_streq(key, "ignore") ||
          f2e_streq(key, "ignored") ||
          f2e_streq(key, "ignore_keys") ||
          f2e_streq(key, "ignored_keys") ||
          f2e_streq(key, "ignore_env") ||
          f2e_streq(key, "ignore_envs") ||
          f2e_streq(key, "ignored_env") ||
          f2e_streq(key, "ignored_envs") ||
          f2e_streq(key, "env_audit_ignore") ||
          f2e_streq(key, "env_audit_ignore_keys")) {
        if (!f2e_parse_env_key_list(config->env_audit_ignored_keys,
                                    &config->env_audit_ignored_count,
                                    value)) {
          config->invalid_env_audit_ignore = 1;
        }
      }
      continue;
    }

    if (section != F2E_SECTION_FLAG || !current) {
      continue;
    }

    if (f2e_streq(key, "env")) {
      char parsed[F2E_MAX_ENV];
      if (f2e_parse_bare_value(value, parsed, sizeof(parsed))) {
        f2e_strlcpy(current->env, parsed, sizeof(current->env));
      }
    } else if (f2e_streq(key, "aliases")) {
      f2e_parse_aliases(current, value);
    } else if (f2e_streq(key, "true_aliases")) {
      f2e_parse_true_aliases(current, value);
    } else if (f2e_streq(key, "false_aliases")) {
      f2e_parse_false_aliases(current, value);
    } else if (f2e_streq(key, "short")) {
      char parsed[F2E_MAX_VALUE];
      if (f2e_parse_bare_value(value, parsed, sizeof(parsed)) && parsed[0] != '\0') {
        current->short_name = parsed[0];
      }
    } else if (f2e_streq(key, "type")) {
      char parsed[F2E_MAX_VALUE];
      if (f2e_parse_bare_value(value, parsed, sizeof(parsed))) {
        f2e_strlcpy(current->type_value, parsed, sizeof(current->type_value));
        current->invalid_type = !f2e_parse_type(value, &current->type);
      } else {
        f2e_strlcpy(current->type_value, value, sizeof(current->type_value));
        current->invalid_type = 1;
      }
    } else if (f2e_streq(key, "default")) {
      char parsed[F2E_MAX_VALUE];
      if (f2e_parse_bare_value(value, parsed, sizeof(parsed))) {
        current->has_default = 1;
        f2e_strlcpy(current->default_value, parsed, sizeof(current->default_value));
      }
    } else if (f2e_streq(key, "help") || f2e_streq(key, "description") || f2e_streq(key, "example")) {
      char parsed[F2E_MAX_VALUE];
      if (f2e_parse_bare_value(value, parsed, sizeof(parsed))) {
        f2e_strlcpy(current->help, parsed, sizeof(current->help));
      }
    }
  }

  fclose(file);
  return 1;
}

static char *f2e_default_config_path(void) {
  char dir[PATH_MAX];
  char home[PATH_MAX];
  const char *pwd = getenv("PWD");
  const char *home_env = getenv("HOME");

#if defined(_WIN32)
  if (GetCurrentDirectoryA(sizeof(dir), dir) == 0) {
    if (pwd && pwd[0] != '\0') {
      f2e_strlcpy(dir, pwd, sizeof(dir));
    } else {
      f2e_strlcpy(dir, ".", sizeof(dir));
    }
  }
#elif defined(__unix__) || defined(__APPLE__)
  if (!getcwd(dir, sizeof(dir))) {
    if (pwd && pwd[0] != '\0') {
      f2e_strlcpy(dir, pwd, sizeof(dir));
    } else {
      f2e_strlcpy(dir, ".", sizeof(dir));
    }
  }
#else
  if (pwd && pwd[0] != '\0') {
    f2e_strlcpy(dir, pwd, sizeof(dir));
  } else {
    f2e_strlcpy(dir, ".", sizeof(dir));
  }
#endif

  if (home_env && home_env[0] != '\0') {
    f2e_strlcpy(home, home_env, sizeof(home));
  } else {
    home[0] = '\0';
  }

  while (dir[0] != '\0') {
    size_t dir_len = strlen(dir);
    while (dir_len > 1 && (dir[dir_len - 1] == '/' || dir[dir_len - 1] == '\\')) {
      dir[--dir_len] = '\0';
    }

    if (home[0] != '\0') {
      size_t home_len = strlen(home);
      while (home_len > 1 && (home[home_len - 1] == '/' || home[home_len - 1] == '\\')) {
        home[--home_len] = '\0';
      }
      if (f2e_streq(dir, home)) {
        return NULL;
      }
    }

    const char suffix[] = "/.cli-flags.toml";
    char *candidate = (char *)malloc(dir_len + sizeof(suffix));
    if (!candidate) {
      return NULL;
    }
    memcpy(candidate, dir, dir_len);
    memcpy(candidate + dir_len, suffix, sizeof(suffix));

    FILE *file = fopen(candidate, "r");
    if (file) {
      fclose(file);
      return candidate;
    }
    free(candidate);

    char *slash = strrchr(dir, '/');
#if defined(_WIN32)
    char *backslash = strrchr(dir, '\\');
    if (!slash || (backslash && backslash > slash)) {
      slash = backslash;
    }
#endif
    if (!slash) {
      break;
    }
    if (slash == dir) {
      dir[1] = '\0';
      if (dir_len == 1) {
        break;
      }
    } else {
      *slash = '\0';
    }
  }

  return NULL;
}

static F2EPair *f2e_find_pair(F2EPair *pairs, size_t pair_count, const char *key) {
  for (size_t i = 0; i < pair_count; i++) {
    if (pairs[i].set && f2e_streq(pairs[i].key, key)) {
      return &pairs[i];
    }
  }
  return NULL;
}

static void f2e_set_pair(F2EPair *pairs, size_t pair_count, const char *key, const char *value) {
  if (!key || key[0] == '\0') {
    return;
  }
  F2EPair *pair = f2e_find_pair(pairs, pair_count, key);
  if (!pair) {
    for (size_t i = 0; i < pair_count; i++) {
      if (!pairs[i].set) {
        pair = &pairs[i];
        pair->set = 1;
        f2e_strlcpy(pair->key, key, sizeof(pair->key));
        break;
      }
    }
  }
  if (pair) {
    f2e_strlcpy(pair->value, value ? value : "", sizeof(pair->value));
  }
}

static int f2e_buffer_init(F2EBuffer *buffer) {
  buffer->cap = 128;
  buffer->len = 0;
  buffer->data = (char *)malloc(buffer->cap);
  if (!buffer->data) {
    return 0;
  }
  buffer->data[0] = '\0';
  return 1;
}

static int f2e_buffer_reserve(F2EBuffer *buffer, size_t extra) {
  if (extra > SIZE_MAX - buffer->len - 1) {
    return 0;
  }
  size_t needed = buffer->len + extra + 1;
  if (needed <= buffer->cap) {
    return 1;
  }
  size_t next = buffer->cap;
  while (needed > next) {
    if (next > SIZE_MAX / 2) {
      next = needed;
      break;
    }
    next *= 2;
  }
  char *data = (char *)realloc(buffer->data, next);
  if (!data) {
    return 0;
  }
  buffer->data = data;
  buffer->cap = next;
  return 1;
}

static int f2e_buffer_append_char(F2EBuffer *buffer, char ch) {
  if (!f2e_buffer_reserve(buffer, 1)) {
    return 0;
  }
  buffer->data[buffer->len++] = ch;
  buffer->data[buffer->len] = '\0';
  return 1;
}

static int f2e_buffer_append(F2EBuffer *buffer, const char *value) {
  size_t len = strlen(value);
  if (!f2e_buffer_reserve(buffer, len)) {
    return 0;
  }
  memcpy(buffer->data + buffer->len, value, len);
  buffer->len += len;
  buffer->data[buffer->len] = '\0';
  return 1;
}

static int f2e_buffer_append_json_string(F2EBuffer *buffer, const char *value) {
  if (!f2e_buffer_append_char(buffer, '"')) {
    return 0;
  }
  for (const unsigned char *cursor = (const unsigned char *)value; *cursor; cursor++) {
    switch (*cursor) {
      case '"':
        if (!f2e_buffer_append(buffer, "\\\"")) {
          return 0;
        }
        break;
      case '\\':
        if (!f2e_buffer_append(buffer, "\\\\")) {
          return 0;
        }
        break;
      case '\b':
        if (!f2e_buffer_append(buffer, "\\b")) {
          return 0;
        }
        break;
      case '\f':
        if (!f2e_buffer_append(buffer, "\\f")) {
          return 0;
        }
        break;
      case '\n':
        if (!f2e_buffer_append(buffer, "\\n")) {
          return 0;
        }
        break;
      case '\r':
        if (!f2e_buffer_append(buffer, "\\r")) {
          return 0;
        }
        break;
      case '\t':
        if (!f2e_buffer_append(buffer, "\\t")) {
          return 0;
        }
        break;
      default:
        if (*cursor < 0x20) {
          char escaped[7];
          snprintf(escaped, sizeof(escaped), "\\u%04x", *cursor);
          if (!f2e_buffer_append(buffer, escaped)) {
            return 0;
          }
        } else if (!f2e_buffer_append_char(buffer, (char)*cursor)) {
          return 0;
        }
        break;
    }
  }
  return f2e_buffer_append_char(buffer, '"');
}

static int f2e_buffer_append_shell_single_quoted(F2EBuffer *buffer, const char *value) {
  if (!f2e_buffer_append_char(buffer, '\'')) {
    return 0;
  }
  for (const char *cursor = value ? value : ""; *cursor; cursor++) {
    if (*cursor == '\'') {
      if (!f2e_buffer_append(buffer, "'\\''")) {
        return 0;
      }
    } else if (!f2e_buffer_append_char(buffer, *cursor)) {
      return 0;
    }
  }
  return f2e_buffer_append_char(buffer, '\'');
}

static int f2e_json_list_init(F2EJsonList *list) {
  memset(list, 0, sizeof(*list));
  if (!f2e_buffer_init(&list->buffer)) {
    list->failed = 1;
    return 0;
  }
  list->initialized = 1;
  if (!f2e_buffer_append_char(&list->buffer, '[')) {
    list->failed = 1;
    return 0;
  }
  return 1;
}

static void f2e_json_list_discard(F2EJsonList *list) {
  if (list && list->initialized) {
    free(list->buffer.data);
  }
  if (list) {
    memset(list, 0, sizeof(*list));
  }
}

static int f2e_json_list_append(F2EJsonList *list, const char *value) {
  if (!list || list->failed || !list->initialized) {
    return 0;
  }
  if (list->count > 0 && !f2e_buffer_append_char(&list->buffer, ',')) {
    list->failed = 1;
    return 0;
  }
  if (!f2e_buffer_append_json_string(&list->buffer, value ? value : "")) {
    list->failed = 1;
    return 0;
  }
  list->count++;
  return 1;
}

static int f2e_json_list_finish(F2EJsonList *list, char *out, size_t out_size) {
  if (!list || list->failed || !list->initialized || !out || out_size == 0) {
    return 0;
  }
  if (!f2e_buffer_append_char(&list->buffer, ']')) {
    list->failed = 1;
    return 0;
  }
  if (list->buffer.len >= out_size) {
    list->failed = 1;
    return 0;
  }
  f2e_strlcpy(out, list->buffer.data, out_size);
  return 1;
}

static int f2e_audit_init(F2EAudit *audit) {
  memset(audit, 0, sizeof(*audit));
  if (!f2e_buffer_init(&audit->errors)) {
    return 0;
  }
  if (!f2e_buffer_init(&audit->warnings)) {
    free(audit->errors.data);
    memset(audit, 0, sizeof(*audit));
    return 0;
  }
  if (!f2e_buffer_append_char(&audit->errors, '[') || !f2e_buffer_append_char(&audit->warnings, '[')) {
    audit->failed = 1;
  }
  return 1;
}

static void f2e_audit_discard(F2EAudit *audit) {
  free(audit->errors.data);
  free(audit->warnings.data);
  memset(audit, 0, sizeof(*audit));
}

static void f2e_audit_add(F2EAudit *audit, int is_error, const char *format, ...) {
  if (!audit || audit->failed) {
    return;
  }

  char message[512];
  va_list args;
  va_start(args, format);
  vsnprintf(message, sizeof(message), format, args);
  va_end(args);

  F2EBuffer *target = is_error ? &audit->errors : &audit->warnings;
  size_t *count = is_error ? &audit->error_count : &audit->warning_count;
  if (*count > 0 && !f2e_buffer_append_char(target, ',')) {
    audit->failed = 1;
    return;
  }
  if (!f2e_buffer_append_json_string(target, message)) {
    audit->failed = 1;
    return;
  }
  (*count)++;
}

static char *f2e_audit_report(F2EAudit *audit, int *status_out) {
  if (!audit || audit->failed || !audit->errors.data || !audit->warnings.data) {
    if (status_out) {
      *status_out = 1;
    }
    if (audit) {
      f2e_audit_discard(audit);
    }
    const char failure_json[] = "{\"ok\":false,\"errorCount\":1,\"warningCount\":0,\"errors\":[\"audit allocation failed\"],\"warnings\":[]}";
    char *failed = (char *)malloc(sizeof(failure_json));
    if (failed) {
      f2e_strlcpy(failed, failure_json, sizeof(failure_json));
    }
    return failed;
  }

  if (!f2e_buffer_append_char(&audit->errors, ']') || !f2e_buffer_append_char(&audit->warnings, ']')) {
    if (status_out) {
      *status_out = 1;
    }
    f2e_audit_discard(audit);
    return f2e_empty_json_object();
  }

  F2EBuffer report;
  if (!f2e_buffer_init(&report)) {
    if (status_out) {
      *status_out = 1;
    }
    f2e_audit_discard(audit);
    return f2e_empty_json_object();
  }

  char counts[96];
  int ok = audit->error_count == 0;
  snprintf(counts, sizeof(counts), "{\"ok\":%s,\"errorCount\":%lu,\"warningCount\":%lu,\"errors\":",
           ok ? "true" : "false",
           (unsigned long)audit->error_count,
           (unsigned long)audit->warning_count);
  if (!f2e_buffer_append(&report, counts) ||
      !f2e_buffer_append(&report, audit->errors.data) ||
      !f2e_buffer_append(&report, ",\"warnings\":") ||
      !f2e_buffer_append(&report, audit->warnings.data) ||
      !f2e_buffer_append_char(&report, '}')) {
    free(report.data);
    if (status_out) {
      *status_out = 1;
    }
    f2e_audit_discard(audit);
    return f2e_empty_json_object();
  }

  if (status_out) {
    *status_out = ok ? 0 : 1;
  }
  f2e_audit_discard(audit);
  return report.data;
}

static char *f2e_pairs_to_json(F2EPair *pairs, size_t pair_count) {
  F2EBuffer buffer;
  if (!f2e_buffer_init(&buffer)) {
    return NULL;
  }
  if (!f2e_buffer_append_char(&buffer, '{')) {
    free(buffer.data);
    return NULL;
  }
  int wrote = 0;
  for (size_t i = 0; i < pair_count; i++) {
    if (!pairs[i].set) {
      continue;
    }
    if (wrote) {
      if (!f2e_buffer_append_char(&buffer, ',')) {
        free(buffer.data);
        return NULL;
      }
    }
    if (!f2e_buffer_append_json_string(&buffer, pairs[i].key) ||
        !f2e_buffer_append_char(&buffer, ':') ||
        !f2e_buffer_append_json_string(&buffer, pairs[i].value)) {
      free(buffer.data);
      return NULL;
    }
    wrote = 1;
  }
  if (!f2e_buffer_append_char(&buffer, '}')) {
    free(buffer.data);
    return NULL;
  }
  return buffer.data;
}

static int f2e_bool_value_alias(const F2EFlag *flag, const char *value, const char **canonical) {
  if (!flag || !value) {
    return 0;
  }
  if (f2e_streq(value, "true")) {
    *canonical = "true";
    return 1;
  }
  if (f2e_streq(value, "false")) {
    *canonical = "false";
    return 1;
  }
  for (size_t i = 0; i < flag->true_alias_count; i++) {
    if (f2e_streq(flag->true_aliases[i], value)) {
      *canonical = "true";
      return 1;
    }
  }
  for (size_t i = 0; i < flag->false_alias_count; i++) {
    if (f2e_streq(flag->false_aliases[i], value)) {
      *canonical = "false";
      return 1;
    }
  }
  return 0;
}

static int f2e_int_value_is_valid(const char *value) {
  if (!value || value[0] == '\0') {
    return 0;
  }
  const char *cursor = value;
  if (*cursor == '+' || *cursor == '-') {
    cursor++;
  }
  if (!isdigit((unsigned char)*cursor)) {
    return 0;
  }
  while (isdigit((unsigned char)*cursor)) {
    cursor++;
  }
  if (*cursor != '\0') {
    return 0;
  }

  errno = 0;
  char *end = NULL;
  (void)strtoll(value, &end, 10);
  return errno != ERANGE && end && *end == '\0';
}

static const char *f2e_value_type_name(F2EValueType type) {
  switch (type) {
    case F2E_TYPE_BOOL:
      return "bool";
    case F2E_TYPE_INT:
      return "integer";
    case F2E_TYPE_JSON:
      return "JSON";
    case F2E_TYPE_STRING:
    default:
      return "string";
  }
}

static int f2e_normalize_flag_value(const F2EFlag *flag, const char *value, char *out, size_t out_size) {
  if (!flag || !value || !out || out_size == 0) {
    return 0;
  }
  switch (flag->type) {
    case F2E_TYPE_BOOL: {
      const char *canonical = NULL;
      if (!f2e_bool_value_alias(flag, value, &canonical)) {
        return 0;
      }
      f2e_strlcpy(out, canonical, out_size);
      return 1;
    }
    case F2E_TYPE_INT:
      if (!f2e_int_value_is_valid(value)) {
        return 0;
      }
      f2e_strlcpy(out, value, out_size);
      return 1;
    case F2E_TYPE_JSON:
      if (!f2e_json_value_is_valid(value)) {
        return 0;
      }
      f2e_strlcpy(out, value, out_size);
      return 1;
    case F2E_TYPE_STRING:
    default:
      f2e_strlcpy(out, value, out_size);
      return 1;
  }
}

static void f2e_report_invalid_value(F2EJsonList *errors, const F2EFlag *flag, const char *value) {
  if (!errors || !errors->initialized || !flag) {
    return;
  }
  char message[512];
  if (flag->type == F2E_TYPE_JSON) {
    snprintf(message, sizeof(message), "flags.%s value \"%s\" is not valid JSON", f2e_audit_flag_name(flag), value ? value : "");
  } else {
    snprintf(message, sizeof(message), "flags.%s value \"%s\" is not a valid %s",
             f2e_audit_flag_name(flag),
             value ? value : "",
             f2e_value_type_name(flag->type));
  }
  f2e_json_list_append(errors, message);
}

static int f2e_try_set_flag_value(F2EFlag *flag, F2EPair *pairs, size_t pair_count, const char *value, F2EJsonList *errors) {
  char normalized[F2E_MAX_VALUE];
  if (!f2e_normalize_flag_value(flag, value, normalized, sizeof(normalized))) {
    f2e_report_invalid_value(errors, flag, value);
    return 0;
  }
  f2e_set_pair(pairs, pair_count, flag->env, normalized);
  return 1;
}

static int f2e_try_set_bool_value(F2EFlag *flag, F2EPair *pairs, size_t pair_count, const char *value) {
  const char *canonical = NULL;
  if (!f2e_bool_value_alias(flag, value, &canonical)) {
    return 0;
  }
  f2e_set_pair(pairs, pair_count, flag->env, canonical);
  return 1;
}

static int f2e_token_looks_like_known_option(F2EConfig *config, const char *token) {
  if (!token || token[0] != '-' || token[1] == '\0') {
    return 0;
  }
  if (token[1] == '-') {
    const char *name = token + 2;
    char copy[F2E_MAX_NAME];
    f2e_strlcpy(copy, name, sizeof(copy));
    char *eq = strchr(copy, '=');
    if (eq) {
      *eq = '\0';
    }
    if (f2e_find_flag_by_alias(config, copy)) {
      return 1;
    }
    if (strncmp(copy, "no-", 3) == 0) {
      F2EFlag *flag = f2e_find_flag_by_alias(config, copy + 3);
      return flag && flag->type == F2E_TYPE_BOOL;
    }
    return 0;
  }
  return f2e_find_flag_by_short(config, token[1]) != NULL;
}

static int f2e_token_looks_like_option(const char *token) {
  return token && token[0] == '-' && token[1] != '\0';
}

static int f2e_can_consume_separated_value(const F2EFlag *flag, const char *token) {
  if (!token || strcmp(token, "--") == 0) {
    return 0;
  }
  if (!f2e_token_looks_like_option(token)) {
    return 1;
  }
  if (flag && flag->type == F2E_TYPE_INT) {
    return f2e_int_value_is_valid(token);
  }
  if (flag && flag->type == F2E_TYPE_JSON) {
    return f2e_json_value_is_valid(token);
  }
  return 0;
}

static int f2e_parse_runtime_bool(const char *value, int *out) {
  if (!value || value[0] == '\0') {
    return 0;
  }
  return f2e_parse_config_bool(value, out);
}

static int f2e_runtime_bool_from_env(const char *name, int *out) {
  const char *value = getenv(name);
  return value && value[0] != '\0' && f2e_parse_runtime_bool(value, out);
}

static int f2e_token_sets_allow_unknown(const char *token, int *out) {
  if (!token) {
    return 0;
  }
  if (f2e_streq(token, "--allow-unknown") || f2e_streq(token, "--allow-hidden")) {
    *out = 1;
    return 1;
  }
  if (f2e_streq(token, "--no-allow-unknown") || f2e_streq(token, "--no-allow-hidden")) {
    *out = 0;
    return 1;
  }

  const char allow_unknown[] = "--allow-unknown=";
  const char allow_hidden[] = "--allow-hidden=";
  if (strncmp(token, allow_unknown, sizeof(allow_unknown) - 1) == 0) {
    return f2e_parse_runtime_bool(token + sizeof(allow_unknown) - 1, out);
  }
  if (strncmp(token, allow_hidden, sizeof(allow_hidden) - 1) == 0) {
    return f2e_parse_runtime_bool(token + sizeof(allow_hidden) - 1, out);
  }
  return 0;
}

static int f2e_resolve_allow_unknown(const F2EConfig *config, int argc, const char *const argv[]) {
  int allow_unknown = config ? config->allow_unknown : 0;
  int parsed = 0;
  if (f2e_runtime_bool_from_env("FLAGS2ENV_ALLOW_UNKNOWN", &parsed) ||
      f2e_runtime_bool_from_env("F2E_ALLOW_UNKNOWN", &parsed) ||
      f2e_runtime_bool_from_env("FLAGS2ENV_ALLOW_HIDDEN", &parsed) ||
      f2e_runtime_bool_from_env("F2E_ALLOW_HIDDEN", &parsed)) {
    allow_unknown = parsed;
  }

  for (int i = 0; i < argc; i++) {
    const char *token = argv && argv[i] ? argv[i] : NULL;
    if (!token) {
      continue;
    }
    if (f2e_streq(token, "--")) {
      break;
    }
    if (f2e_token_sets_allow_unknown(token, &parsed)) {
      allow_unknown = parsed;
    }
  }
  return allow_unknown;
}

static void f2e_apply_defaults(F2EConfig *config, F2EPair *pairs, size_t pair_count) {
  for (size_t i = 0; i < config->flag_count; i++) {
    F2EFlag *flag = &config->flags[i];
    if (flag->env[0] != '\0' && flag->has_default) {
      char normalized[F2E_MAX_VALUE];
      if (f2e_normalize_flag_value(flag, flag->default_value, normalized, sizeof(normalized))) {
        f2e_set_pair(pairs, pair_count, flag->env, normalized);
      }
    }
  }
}

static int f2e_can_bundle_bool_shorts(F2EConfig *config, const char *shorts) {
  if (!shorts || shorts[0] == '\0') {
    return 0;
  }
  for (const char *cursor = shorts; *cursor; cursor++) {
    F2EFlag *flag = f2e_find_flag_by_short(config, *cursor);
    if (!flag || flag->env[0] == '\0' || flag->type != F2E_TYPE_BOOL) {
      return 0;
    }
  }
  return 1;
}

static void f2e_apply_bool_short_bundle(F2EConfig *config, F2EPair *pairs, size_t pair_count, const char *shorts) {
  for (const char *cursor = shorts; *cursor; cursor++) {
    F2EFlag *flag = f2e_find_flag_by_short(config, *cursor);
    if (flag && flag->env[0] != '\0' && flag->type == F2E_TYPE_BOOL) {
      f2e_set_pair(pairs, pair_count, flag->env, "true");
    }
  }
}

static void f2e_apply_long_arg(F2EConfig *config, F2EPair *pairs, size_t pair_count, const char *token, int *index, int argc, const char *const argv[], F2EJsonList *errors) {
  char name[F2E_MAX_NAME];
  char inline_value[F2E_MAX_VALUE];
  int has_inline_value = 0;
  int negated = 0;

  const char *raw = token + 2;
  f2e_strlcpy(name, raw, sizeof(name));
  char *eq = strchr(name, '=');
  if (eq) {
    *eq = '\0';
    f2e_strlcpy(inline_value, eq + 1, sizeof(inline_value));
    has_inline_value = 1;
  }

  F2EFlag *flag = f2e_find_flag_by_alias(config, name);
  if (!flag && strncmp(name, "no-", 3) == 0) {
    flag = f2e_find_flag_by_alias(config, name + 3);
    if (flag && flag->type == F2E_TYPE_BOOL) {
      negated = 1;
    } else {
      return;
    }
  }
  if (!flag || flag->env[0] == '\0') {
    return;
  }

  if (flag->type == F2E_TYPE_BOOL) {
    if (negated) {
      f2e_set_pair(pairs, pair_count, flag->env, "false");
    } else if (has_inline_value) {
      f2e_try_set_flag_value(flag, pairs, pair_count, inline_value, errors);
    } else if (config->allow_separated_values &&
               *index + 1 < argc &&
               f2e_can_consume_separated_value(flag, argv[*index + 1]) &&
               f2e_try_set_bool_value(flag, pairs, pair_count, argv[*index + 1])) {
      (*index)++;
    } else {
      f2e_set_pair(pairs, pair_count, flag->env, "true");
    }
    return;
  }

  if (has_inline_value) {
    f2e_try_set_flag_value(flag, pairs, pair_count, inline_value, errors);
  } else if (config->allow_separated_values &&
             *index + 1 < argc &&
             f2e_can_consume_separated_value(flag, argv[*index + 1])) {
    (*index)++;
    f2e_try_set_flag_value(flag, pairs, pair_count, argv[*index], errors);
  }
}

static void f2e_apply_short_arg(F2EConfig *config, F2EPair *pairs, size_t pair_count, const char *token, int *index, int argc, const char *const argv[], F2EJsonList *errors) {
  if (token[1] == '\0') {
    return;
  }

  char short_name = token[1];
  F2EFlag *first = f2e_find_flag_by_short(config, short_name);
  if (!first || first->env[0] == '\0') {
    return;
  }

  const char *rest = token + 2;
  int has_inline_value = 0;
  if (*rest == '=') {
    has_inline_value = 1;
    rest++;
  }

  if (first->type != F2E_TYPE_BOOL) {
    if (*rest) {
      f2e_try_set_flag_value(first, pairs, pair_count, rest, errors);
    } else if (config->allow_separated_values &&
               *index + 1 < argc &&
               f2e_can_consume_separated_value(first, argv[*index + 1])) {
      (*index)++;
      f2e_try_set_flag_value(first, pairs, pair_count, argv[*index], errors);
    }
    return;
  }

  if (has_inline_value) {
    f2e_try_set_flag_value(first, pairs, pair_count, rest, errors);
    return;
  }

  if (*rest == '\0') {
    if (config->allow_separated_values &&
        *index + 1 < argc &&
        f2e_can_consume_separated_value(first, argv[*index + 1]) &&
        f2e_try_set_bool_value(first, pairs, pair_count, argv[*index + 1])) {
      (*index)++;
      return;
    }
    f2e_set_pair(pairs, pair_count, first->env, "true");
    return;
  }

  if (f2e_can_bundle_bool_shorts(config, token + 1)) {
    f2e_apply_bool_short_bundle(config, pairs, pair_count, token + 1);
    return;
  }

  f2e_try_set_flag_value(first, pairs, pair_count, rest, errors);
}

static const char *f2e_audit_flag_name(const F2EFlag *flag) {
  return flag && flag->name[0] != '\0' ? flag->name : "<unnamed>";
}

static void f2e_audit_bool_value_aliases(const F2EFlag *flag, F2EAudit *audit) {
  if (flag->type != F2E_TYPE_BOOL) {
    if (flag->true_alias_count > 0 || flag->false_alias_count > 0) {
      f2e_audit_add(audit, 0, "flags.%s declares boolean value aliases but type is not bool", f2e_audit_flag_name(flag));
    }
    if (flag->has_default && flag->type == F2E_TYPE_INT && !f2e_int_value_is_valid(flag->default_value)) {
      f2e_audit_add(audit, 1, "flags.%s default \"%s\" is not a valid integer",
                    f2e_audit_flag_name(flag),
                    flag->default_value);
    }
    if (flag->has_default && flag->type == F2E_TYPE_JSON && !f2e_json_value_is_valid(flag->default_value)) {
      f2e_audit_add(audit, 1, "flags.%s default \"%s\" is not valid JSON",
                    f2e_audit_flag_name(flag),
                    flag->default_value);
    }
    return;
  }

  for (size_t i = 0; i < flag->true_alias_count; i++) {
    if (!f2e_shell_word_is_valid(flag->true_aliases[i])) {
      f2e_audit_add(audit, 1, "flags.%s true_aliases contains unsafe shell token \"%s\"",
                    f2e_audit_flag_name(flag),
                    flag->true_aliases[i]);
    }
    if (f2e_streq(flag->true_aliases[i], "false")) {
      f2e_audit_add(audit, 1, "flags.%s true_aliases contains canonical false", f2e_audit_flag_name(flag));
    } else if (f2e_streq(flag->true_aliases[i], "true")) {
      f2e_audit_add(audit, 0, "flags.%s true_aliases redundantly contains canonical true", f2e_audit_flag_name(flag));
    }
  }

  for (size_t i = 0; i < flag->false_alias_count; i++) {
    if (!f2e_shell_word_is_valid(flag->false_aliases[i])) {
      f2e_audit_add(audit, 1, "flags.%s false_aliases contains unsafe shell token \"%s\"",
                    f2e_audit_flag_name(flag),
                    flag->false_aliases[i]);
    }
    if (f2e_streq(flag->false_aliases[i], "true")) {
      f2e_audit_add(audit, 1, "flags.%s false_aliases contains canonical true", f2e_audit_flag_name(flag));
    } else if (f2e_streq(flag->false_aliases[i], "false")) {
      f2e_audit_add(audit, 0, "flags.%s false_aliases redundantly contains canonical false", f2e_audit_flag_name(flag));
    }
  }

  for (size_t i = 0; i < flag->true_alias_count; i++) {
    for (size_t j = 0; j < flag->false_alias_count; j++) {
      if (f2e_streq(flag->true_aliases[i], flag->false_aliases[j])) {
        f2e_audit_add(audit, 1, "flags.%s value alias \"%s\" appears in both true_aliases and false_aliases",
                      f2e_audit_flag_name(flag),
                      flag->true_aliases[i]);
      }
    }
  }

  if (flag->has_default) {
    const char *canonical = NULL;
    if (!f2e_bool_value_alias(flag, flag->default_value, &canonical)) {
      f2e_audit_add(audit, 1, "flags.%s default \"%s\" is not a valid bool value",
                    f2e_audit_flag_name(flag),
                    flag->default_value);
    }
  }
}

static void f2e_audit_config_semantics(const F2EConfig *config, F2EAudit *audit) {
  if (config->flag_count == 0) {
    f2e_audit_add(audit, 1, "no [flags.*] tables declared");
    return;
  }

  for (size_t i = 0; i < config->flag_count; i++) {
    const F2EFlag *flag = &config->flags[i];
    if (flag->name[0] == '\0') {
      f2e_audit_add(audit, 1, "flags.%lu has empty name", (unsigned long)i);
    }
    if (flag->env[0] == '\0') {
      f2e_audit_add(audit, 1, "flags.%s is missing env", f2e_audit_flag_name(flag));
    } else if (!f2e_env_name_is_valid(flag->env)) {
      f2e_audit_add(audit, 1, "flags.%s env \"%s\" is not a valid env var name", f2e_audit_flag_name(flag), flag->env);
    }
    if (flag->alias_count == 0) {
      f2e_audit_add(audit, 1, "flags.%s has no long aliases", f2e_audit_flag_name(flag));
    }
    for (size_t j = 0; j < flag->alias_count; j++) {
      const char *alias = flag->aliases[j];
      if (alias[0] == '\0') {
        f2e_audit_add(audit, 1, "flags.%s contains an empty alias", f2e_audit_flag_name(flag));
      } else if (alias[0] == '-') {
        f2e_audit_add(audit, 1, "flags.%s alias \"%s\" should not include leading dashes", f2e_audit_flag_name(flag), alias);
      } else if (!f2e_option_name_is_valid(alias)) {
        f2e_audit_add(audit, 1, "flags.%s alias \"%s\" contains unsafe option characters", f2e_audit_flag_name(flag), alias);
      }
    }
    if (flag->short_name != '\0' && !isalnum((unsigned char)flag->short_name)) {
      f2e_audit_add(audit, 1, "flags.%s has invalid short flag \"%c\"", f2e_audit_flag_name(flag), flag->short_name);
    }
    if (flag->invalid_type) {
      f2e_audit_add(audit, 1, "flags.%s type \"%s\" is not supported",
                    f2e_audit_flag_name(flag),
                    flag->type_value);
    }
    f2e_audit_bool_value_aliases(flag, audit);

    if (config->positionals_env[0] != '\0' && f2e_streq(config->positionals_env, flag->env)) {
      f2e_audit_add(audit, 1, "parse.positionals_env collides with flags.%s env \"%s\"",
                    f2e_audit_flag_name(flag),
                    config->positionals_env);
    }
    if (config->unknown_options_env[0] != '\0' && f2e_streq(config->unknown_options_env, flag->env)) {
      f2e_audit_add(audit, 1, "parse.unknown_options_env collides with flags.%s env \"%s\"",
                    f2e_audit_flag_name(flag),
                    config->unknown_options_env);
    }
    if (config->errors_env[0] != '\0' && f2e_streq(config->errors_env, flag->env)) {
      f2e_audit_add(audit, 1, "parse.errors_env collides with flags.%s env \"%s\"",
                    f2e_audit_flag_name(flag),
                    config->errors_env);
    }
  }

  if (config->positionals_env[0] != '\0' &&
      !f2e_env_name_is_valid(config->positionals_env)) {
    f2e_audit_add(audit, 1, "parse.positionals_env \"%s\" is not a valid env var name",
                  config->positionals_env);
  }
  if (config->unknown_options_env[0] != '\0' &&
      !f2e_env_name_is_valid(config->unknown_options_env)) {
    f2e_audit_add(audit, 1, "parse.unknown_options_env \"%s\" is not a valid env var name",
                  config->unknown_options_env);
  }
  if (config->errors_env[0] != '\0' &&
      !f2e_env_name_is_valid(config->errors_env)) {
    f2e_audit_add(audit, 1, "parse.errors_env \"%s\" is not a valid env var name",
                  config->errors_env);
  }
  if (config->invalid_help_columns) {
    f2e_audit_add(audit, 1, "help.columns must be a list of supported table column names");
  }
  if (config->invalid_help_exclude_columns) {
    f2e_audit_add(audit, 1, "help.exclude must be a list of supported table column names");
  }
  if (config->invalid_env_audit_ignore) {
    f2e_audit_add(audit, 1, "env.ignore must be a list of env var names");
  }
  for (size_t i = 0; i < config->env_audit_ignored_count; i++) {
    if (!f2e_env_name_is_valid(config->env_audit_ignored_keys[i])) {
      f2e_audit_add(audit, 1, "env.ignore contains invalid env var name \"%s\"",
                    config->env_audit_ignored_keys[i]);
    }
  }

  if (config->positionals_env[0] != '\0' &&
      config->unknown_options_env[0] != '\0' &&
      f2e_streq(config->positionals_env, config->unknown_options_env)) {
    f2e_audit_add(audit, 1, "parse.positionals_env and parse.unknown_options_env both use env \"%s\"",
                  config->positionals_env);
  }
  if (config->positionals_env[0] != '\0' &&
      config->errors_env[0] != '\0' &&
      f2e_streq(config->positionals_env, config->errors_env)) {
    f2e_audit_add(audit, 1, "parse.positionals_env and parse.errors_env both use env \"%s\"",
                  config->positionals_env);
  }
  if (config->unknown_options_env[0] != '\0' &&
      config->errors_env[0] != '\0' &&
      f2e_streq(config->unknown_options_env, config->errors_env)) {
    f2e_audit_add(audit, 1, "parse.unknown_options_env and parse.errors_env both use env \"%s\"",
                  config->unknown_options_env);
  }

  for (size_t i = 0; i < config->flag_count; i++) {
    const F2EFlag *left = &config->flags[i];

    if (left->type == F2E_TYPE_BOOL) {
      for (size_t alias_index = 0; alias_index < left->alias_count; alias_index++) {
        char negated_alias[F2E_MAX_NAME + 3];
        snprintf(negated_alias, sizeof(negated_alias), "no-%s", left->aliases[alias_index]);
        const F2EFlag *clash = f2e_find_flag_by_alias_const(config, negated_alias);
        if (clash) {
          f2e_audit_add(audit, 1, "alias \"%s\" clashes with negated bool flag flags.%s",
                        negated_alias,
                        f2e_audit_flag_name(left));
        }
      }
    }

    for (size_t j = i + 1; j < config->flag_count; j++) {
      const F2EFlag *right = &config->flags[j];
      if (left->env[0] != '\0' && right->env[0] != '\0' && f2e_streq(left->env, right->env)) {
        f2e_audit_add(audit, 1, "flags.%s and flags.%s both map to env \"%s\"",
                      f2e_audit_flag_name(left),
                      f2e_audit_flag_name(right),
                      left->env);
      }
      if (left->short_name != '\0' && right->short_name != '\0' && left->short_name == right->short_name) {
        f2e_audit_add(audit, 1, "flags.%s and flags.%s both use short flag \"%c\"",
                      f2e_audit_flag_name(left),
                      f2e_audit_flag_name(right),
                      left->short_name);
      }
      for (size_t left_alias = 0; left_alias < left->alias_count; left_alias++) {
        for (size_t right_alias = 0; right_alias < right->alias_count; right_alias++) {
          if (f2e_streq(left->aliases[left_alias], right->aliases[right_alias])) {
            f2e_audit_add(audit, 1, "flags.%s and flags.%s both use alias \"%s\"",
                          f2e_audit_flag_name(left),
                          f2e_audit_flag_name(right),
                          left->aliases[left_alias]);
          }
        }
      }
    }
  }
}

static int f2e_config_has_audit_errors(const F2EConfig *config) {
  F2EAudit audit;
  if (!f2e_audit_init(&audit)) {
    return 1;
  }
  f2e_audit_config_semantics(config, &audit);
  int has_errors = audit.failed || audit.error_count > 0;
  f2e_audit_discard(&audit);
  return has_errors;
}

static char *f2e_audit_error_report(const char *message, int *status_out) {
  F2EAudit audit;
  if (!f2e_audit_init(&audit)) {
    if (status_out) {
      *status_out = 1;
    }
    return f2e_empty_json_object();
  }
  f2e_audit_add(&audit, 1, "%s", message);
  return f2e_audit_report(&audit, status_out);
}

static char *f2e_audit_config_from_file_impl(const char *config_path, int *status_out) {
  F2EAudit audit;
  if (!f2e_audit_init(&audit)) {
    if (status_out) {
      *status_out = 1;
    }
    return f2e_empty_json_object();
  }

  F2EConfig *config = (F2EConfig *)malloc(sizeof(F2EConfig));
  if (!config) {
    f2e_audit_add(&audit, 1, "audit allocation failed");
    return f2e_audit_report(&audit, status_out);
  }

  if (!config_path || config_path[0] == '\0') {
    f2e_audit_add(&audit, 1, "config path is empty");
  } else if (!f2e_load_config(config_path, config)) {
    f2e_audit_add(&audit, 1, "could not read config \"%s\"", config_path);
  } else {
    f2e_audit_config_semantics(config, &audit);
  }

  free(config);
  return f2e_audit_report(&audit, status_out);
}

const char *f2e_version(void) {
  return F2E_VERSION;
}

char *f2e_audit_config_from_file(const char *config_path) {
  return f2e_audit_config_from_file_impl(config_path, NULL);
}

char *f2e_audit_config(void) {
  char *path = f2e_default_config_path();
  if (!path) {
    return f2e_audit_error_report("no usable .cli-flags.toml found before HOME", NULL);
  }
  char *result = f2e_audit_config_from_file(path);
  free(path);
  return result;
}

int f2e_audit_config_status_from_file(const char *config_path) {
  int status = 1;
  char *report = f2e_audit_config_from_file_impl(config_path, &status);
  free(report);
  return status;
}

int f2e_audit_config_status(void) {
  int status = 1;
  char *path = f2e_default_config_path();
  if (!path) {
    return 1;
  }
  char *report = f2e_audit_config_from_file_impl(path, &status);
  free(report);
  free(path);
  return status;
}

typedef struct {
  char keys[F2E_MAX_ENV_FILE_KEYS][F2E_MAX_ENV];
  size_t count;
} F2EEnvKeySet;

static char *f2e_sibling_path(const char *path, const char *file_name) {
  if (!path || !file_name || file_name[0] == '\0') {
    return NULL;
  }

  const char *slash = strrchr(path, '/');
#if defined(_WIN32)
  const char *backslash = strrchr(path, '\\');
  if (!slash || (backslash && backslash > slash)) {
    slash = backslash;
  }
#endif

  if (!slash) {
    return f2e_strdup(file_name);
  }

  size_t dir_len = (size_t)(slash - path);
  char separator = *slash;
  if (dir_len == 0) {
    dir_len = 1;
  }

  size_t file_len = strlen(file_name);
  size_t needs_separator = path[dir_len - 1] == '/' || path[dir_len - 1] == '\\' ? 0 : 1;
  if (dir_len > SIZE_MAX - needs_separator - file_len - 1) {
    return NULL;
  }

  char *joined = (char *)malloc(dir_len + needs_separator + file_len + 1);
  if (!joined) {
    return NULL;
  }
  memcpy(joined, path, dir_len);
  size_t offset = dir_len;
  if (needs_separator) {
    joined[offset++] = separator;
  }
  memcpy(joined + offset, file_name, file_len);
  joined[offset + file_len] = '\0';
  return joined;
}

static int f2e_env_key_is_valid(const char *key) {
  return f2e_env_name_is_valid(key);
}

static int f2e_env_keyset_contains(const F2EEnvKeySet *set, const char *key) {
  if (!set || !key) {
    return 0;
  }
  for (size_t i = 0; i < set->count; i++) {
    if (f2e_streq(set->keys[i], key)) {
      return 1;
    }
  }
  return 0;
}

static int f2e_env_keyset_add(F2EEnvKeySet *set, const char *key, int *duplicate_out) {
  if (duplicate_out) {
    *duplicate_out = 0;
  }
  if (!set || !key || key[0] == '\0') {
    return 1;
  }
  if (f2e_env_keyset_contains(set, key)) {
    if (duplicate_out) {
      *duplicate_out = 1;
    }
    return 1;
  }
  if (set->count >= F2E_MAX_ENV_FILE_KEYS) {
    return 0;
  }
  f2e_strlcpy(set->keys[set->count++], key, sizeof(set->keys[0]));
  return 1;
}

static int f2e_config_ignores_env_key(const F2EConfig *config, const char *key) {
  if (!config || !key) {
    return 0;
  }
  for (size_t i = 0; i < config->env_audit_ignored_count; i++) {
    if (f2e_streq(config->env_audit_ignored_keys[i], key)) {
      return 1;
    }
  }
  return 0;
}

static void f2e_collect_config_env_keys(const F2EConfig *config, F2EEnvKeySet *declared, int include_ignored) {
  memset(declared, 0, sizeof(*declared));
  if (!config) {
    return;
  }

  for (size_t i = 0; i < config->flag_count; i++) {
    int duplicate = 0;
    if (include_ignored || !f2e_config_ignores_env_key(config, config->flags[i].env)) {
      f2e_env_keyset_add(declared, config->flags[i].env, &duplicate);
    }
  }
  int duplicate = 0;
  if (include_ignored || !f2e_config_ignores_env_key(config, config->positionals_env)) {
    f2e_env_keyset_add(declared, config->positionals_env, &duplicate);
  }
  if (include_ignored || !f2e_config_ignores_env_key(config, config->unknown_options_env)) {
    f2e_env_keyset_add(declared, config->unknown_options_env, &duplicate);
  }
  if (include_ignored || !f2e_config_ignores_env_key(config, config->errors_env)) {
    f2e_env_keyset_add(declared, config->errors_env, &duplicate);
  }
}

static void f2e_audit_env_file_semantics(const F2EConfig *config, const char *env_path, F2EAudit *audit) {
  F2EEnvKeySet declared;
  F2EEnvKeySet declared_all;
  F2EEnvKeySet seen;
  f2e_collect_config_env_keys(config, &declared, 0);
  f2e_collect_config_env_keys(config, &declared_all, 1);
  memset(&seen, 0, sizeof(seen));

  if (declared_all.count == 0) {
    f2e_audit_add(audit, 1, ".cli-flags.toml declares no env keys");
    return;
  }

  FILE *file = fopen(env_path, "r");
  if (!file) {
    f2e_audit_add(audit, 1, "could not read env file \"%s\"", env_path ? env_path : "");
    return;
  }

  char line[F2E_MAX_LINE];
  unsigned long line_no = 0;
  while (fgets(line, sizeof(line), file)) {
    line_no++;
    char *trimmed = f2e_trim(line);
    if (trimmed[0] == '\0' || trimmed[0] == '#') {
      continue;
    }
    if (strncmp(trimmed, "export", 6) == 0 && isspace((unsigned char)trimmed[6])) {
      trimmed = f2e_trim_left(trimmed + 6);
    }

    char *eq = strchr(trimmed, '=');
    if (!eq) {
      f2e_audit_add(audit, 0, ".env line %lu is not KEY=value", line_no);
      continue;
    }
    *eq = '\0';
    char *key = f2e_trim(trimmed);
    if (!f2e_env_key_is_valid(key)) {
      f2e_audit_add(audit, 0, ".env line %lu has invalid key \"%s\"", line_no, key);
      continue;
    }
    if (f2e_config_ignores_env_key(config, key)) {
      continue;
    }

    int duplicate = 0;
    if (!f2e_env_keyset_add(&seen, key, &duplicate)) {
      f2e_audit_add(audit, 1, ".env declares too many keys");
      break;
    }
    if (duplicate) {
      f2e_audit_add(audit, 0, ".env key \"%s\" appears more than once", key);
      continue;
    }
    if (!f2e_env_keyset_contains(&declared, key)) {
      f2e_audit_add(audit, 1, ".env key \"%s\" is not declared by .cli-flags.toml", key);
    }
  }

  fclose(file);

  for (size_t i = 0; i < declared.count; i++) {
    if (!f2e_env_keyset_contains(&seen, declared.keys[i])) {
      f2e_audit_add(audit, 0, ".cli-flags.toml env \"%s\" is not present in .env", declared.keys[i]);
    }
  }
}

static char *f2e_audit_env_file_from_file_impl(const char *config_path, const char *env_path, int *status_out) {
  F2EAudit audit;
  if (!f2e_audit_init(&audit)) {
    if (status_out) {
      *status_out = 1;
    }
    return f2e_empty_json_object();
  }

  F2EConfig *config = (F2EConfig *)malloc(sizeof(F2EConfig));
  if (!config) {
    f2e_audit_add(&audit, 1, "audit allocation failed");
    return f2e_audit_report(&audit, status_out);
  }

  char *resolved_env_path = NULL;
  if (!config_path || config_path[0] == '\0') {
    f2e_audit_add(&audit, 1, "config path is empty");
  } else if (!f2e_load_config(config_path, config)) {
    f2e_audit_add(&audit, 1, "could not read config \"%s\"", config_path);
  } else {
    size_t config_error_count = audit.error_count;
    f2e_audit_config_semantics(config, &audit);
    if (!audit.failed && audit.error_count == config_error_count) {
      resolved_env_path = env_path && env_path[0] != '\0' ? f2e_strdup(env_path) : f2e_sibling_path(config_path, ".env");
      if (!resolved_env_path) {
        f2e_audit_add(&audit, 1, "env path allocation failed");
      } else {
        f2e_audit_env_file_semantics(config, resolved_env_path, &audit);
      }
    }
  }

  free(resolved_env_path);
  free(config);
  return f2e_audit_report(&audit, status_out);
}

char *f2e_audit_env_file_from_file(const char *config_path, const char *env_path) {
  return f2e_audit_env_file_from_file_impl(config_path, env_path, NULL);
}

char *f2e_audit_env_file(void) {
  char *path = f2e_default_config_path();
  if (!path) {
    return f2e_audit_error_report("no usable .cli-flags.toml found before HOME", NULL);
  }
  char *result = f2e_audit_env_file_from_file(path, NULL);
  free(path);
  return result;
}

int f2e_audit_env_file_status_from_file(const char *config_path, const char *env_path) {
  int status = 1;
  char *report = f2e_audit_env_file_from_file_impl(config_path, env_path, &status);
  free(report);
  return status;
}

int f2e_audit_env_file_status(void) {
  int status = 1;
  char *path = f2e_default_config_path();
  if (!path) {
    return 1;
  }
  char *report = f2e_audit_env_file_from_file_impl(path, NULL, &status);
  free(report);
  free(path);
  return status;
}

static int f2e_completion_append_word(F2EBuffer *words, const char *word) {
  if (!word || word[0] == '\0') {
    return 1;
  }
  if (words->len > 0 && !f2e_buffer_append_char(words, ' ')) {
    return 0;
  }
  return f2e_buffer_append(words, word);
}

static int f2e_completion_command_name(const char *command_name, char *out, size_t out_size) {
  return f2e_path_basename_copy(command_name, out, out_size);
}

static void f2e_completion_function_name(const char *command_name, char *out, size_t out_size) {
  const char *command = command_name && command_name[0] != '\0' ? command_name : "flags2env";
  const char prefix[] = "_flags2env_complete_";
  if (out_size == 0) {
    return;
  }
  f2e_strlcpy(out, prefix, out_size);
  size_t len = strlen(out);
  for (const unsigned char *cursor = (const unsigned char *)command; *cursor && len + 1 < out_size; cursor++) {
    out[len++] = isalnum(*cursor) ? (char)*cursor : '_';
  }
  out[len] = '\0';
  if (len == sizeof(prefix) - 1 && len + 7 < out_size) {
    f2e_strlcpy(out + len, "command", out_size - len);
  }
}

static int f2e_completion_add_option_word(F2EBuffer *all_options, const char *prefix, const char *name, const char *suffix) {
  if (!f2e_option_name_is_valid(name)) {
    return 0;
  }
  char option[F2E_MAX_NAME + 8];
  snprintf(option, sizeof(option), "%s%s%s", prefix, name, suffix ? suffix : "");
  return f2e_completion_append_word(all_options, option);
}

static int f2e_completion_add_bool_values(F2EBuffer *bool_values, const F2EFlag *flag) {
  if (!f2e_completion_append_word(bool_values, "true") ||
      !f2e_completion_append_word(bool_values, "false")) {
    return 0;
  }
  for (size_t i = 0; i < flag->true_alias_count; i++) {
    if (!f2e_shell_word_is_valid(flag->true_aliases[i])) {
      return 0;
    }
    if (!f2e_completion_append_word(bool_values, flag->true_aliases[i])) {
      return 0;
    }
  }
  for (size_t i = 0; i < flag->false_alias_count; i++) {
    if (!f2e_shell_word_is_valid(flag->false_aliases[i])) {
      return 0;
    }
    if (!f2e_completion_append_word(bool_values, flag->false_aliases[i])) {
      return 0;
    }
  }
  return 1;
}

static int f2e_completion_collect_bash_words(const F2EConfig *config,
                                             F2EBuffer *all_options,
                                             F2EBuffer *value_options,
                                             F2EBuffer *bool_value_options,
                                             F2EBuffer *bool_values) {
  if (!f2e_buffer_init(all_options) ||
      !f2e_buffer_init(value_options) ||
      !f2e_buffer_init(bool_value_options) ||
      !f2e_buffer_init(bool_values)) {
    return 0;
  }

  for (size_t i = 0; i < config->flag_count; i++) {
    const F2EFlag *flag = &config->flags[i];
    if (flag->env[0] == '\0') {
      continue;
    }
    for (size_t j = 0; j < flag->alias_count; j++) {
      if (!f2e_option_name_is_valid(flag->aliases[j])) {
        return 0;
      }
      char option[F2E_MAX_NAME + 4];
      snprintf(option, sizeof(option), "--%s", flag->aliases[j]);
      if (!f2e_completion_append_word(all_options, option)) {
        return 0;
      }
      if (flag->type == F2E_TYPE_BOOL) {
        if (!f2e_completion_append_word(bool_value_options, option) ||
            !f2e_completion_add_option_word(all_options, "--no-", flag->aliases[j], NULL)) {
          return 0;
        }
      } else {
        if (!f2e_completion_append_word(value_options, option) ||
            !f2e_completion_add_option_word(all_options, "--", flag->aliases[j], "=")) {
          return 0;
        }
      }
    }

    if (flag->short_name != '\0') {
      if (!isalnum((unsigned char)flag->short_name)) {
        return 0;
      }
      char short_option[4] = {'-', flag->short_name, '\0', '\0'};
      if (!f2e_completion_append_word(all_options, short_option)) {
        return 0;
      }
      if (flag->type == F2E_TYPE_BOOL) {
        if (!f2e_completion_append_word(bool_value_options, short_option)) {
          return 0;
        }
      } else if (!f2e_completion_append_word(value_options, short_option)) {
        return 0;
      }
    }

    if (flag->type == F2E_TYPE_BOOL && !f2e_completion_add_bool_values(bool_values, flag)) {
      return 0;
    }
  }

  return 1;
}

static void f2e_completion_free_words(F2EBuffer *a, F2EBuffer *b, F2EBuffer *c, F2EBuffer *d) {
  free(a->data);
  free(b->data);
  free(c->data);
  free(d->data);
}

static char *f2e_completion_script_bash(const F2EConfig *config, const char *command_name) {
  F2EBuffer options;
  F2EBuffer value_options;
  F2EBuffer bool_value_options;
  F2EBuffer bool_values;
  memset(&options, 0, sizeof(options));
  memset(&value_options, 0, sizeof(value_options));
  memset(&bool_value_options, 0, sizeof(bool_value_options));
  memset(&bool_values, 0, sizeof(bool_values));
  if (!f2e_completion_collect_bash_words(config, &options, &value_options, &bool_value_options, &bool_values)) {
    f2e_completion_free_words(&options, &value_options, &bool_value_options, &bool_values);
    return NULL;
  }

  F2EBuffer script;
  if (!f2e_buffer_init(&script)) {
    f2e_completion_free_words(&options, &value_options, &bool_value_options, &bool_values);
    return NULL;
  }

  char command[F2E_MAX_NAME];
  if (!f2e_completion_command_name(command_name, command, sizeof(command))) {
    f2e_completion_free_words(&options, &value_options, &bool_value_options, &bool_values);
    free(script.data);
    return NULL;
  }
  char function_name[F2E_MAX_NAME * 2];
  f2e_completion_function_name(command, function_name, sizeof(function_name));

  if (!f2e_buffer_append(&script, "# flags2env bash completion\n") ||
      !f2e_buffer_append(&script, function_name) ||
      !f2e_buffer_append(&script, "() {\n"
                                  "  local cur prev opt opts value_opts bool_value_opts bool_values\n"
                                  "  COMPREPLY=()\n"
                                  "  cur=\"${COMP_WORDS[COMP_CWORD]}\"\n"
                                  "  prev=\"${COMP_WORDS[COMP_CWORD-1]}\"\n"
                                  "  opts=") ||
      !f2e_buffer_append_shell_single_quoted(&script, options.data) ||
      !f2e_buffer_append(&script, "\n  value_opts=") ||
      !f2e_buffer_append_shell_single_quoted(&script, value_options.data) ||
      !f2e_buffer_append(&script, "\n  bool_value_opts=") ||
      !f2e_buffer_append_shell_single_quoted(&script, bool_value_options.data) ||
      !f2e_buffer_append(&script, "\n  bool_values=") ||
      !f2e_buffer_append_shell_single_quoted(&script, bool_values.data) ||
      !f2e_buffer_append(&script, "\n"
                                  "  for opt in $bool_value_opts; do\n"
                                  "    if [ \"$prev\" = \"$opt\" ]; then\n"
                                  "      COMPREPLY=( $(compgen -W \"$bool_values\" -- \"$cur\") )\n"
                                  "      return 0\n"
                                  "    fi\n"
                                  "  done\n"
                                  "  for opt in $value_opts; do\n"
                                  "    if [ \"$prev\" = \"$opt\" ]; then\n"
                                  "      return 0\n"
                                  "    fi\n"
                                  "  done\n"
                                  "  case \"$cur\" in\n"
                                  "    -*) COMPREPLY=( $(compgen -W \"$opts\" -- \"$cur\") ) ;;\n"
                                  "  esac\n"
                                  "  return 0\n"
                                  "}\n"
                                  "complete -o default -F ") ||
      !f2e_buffer_append(&script, function_name) ||
      !f2e_buffer_append(&script, " -- ") ||
      !f2e_buffer_append_shell_single_quoted(&script, command) ||
      !f2e_buffer_append_char(&script, '\n')) {
    free(script.data);
    f2e_completion_free_words(&options, &value_options, &bool_value_options, &bool_values);
    return NULL;
  }

  f2e_completion_free_words(&options, &value_options, &bool_value_options, &bool_values);
  return script.data;
}

static int f2e_completion_zsh_append_spec(F2EBuffer *script, const F2EBuffer *spec) {
  return f2e_buffer_append(script, "    ") &&
         f2e_buffer_append_shell_single_quoted(script, spec->data) &&
         f2e_buffer_append(script, " \\\n");
}

static int f2e_completion_zsh_option_spec(F2EBuffer *script, const char *option, const F2EFlag *flag, int bool_negated) {
  F2EBuffer spec;
  F2EBuffer values;
  if (!f2e_buffer_init(&spec)) {
    return 0;
  }
  memset(&values, 0, sizeof(values));
  if (flag->env[0] != '\0' && !f2e_env_name_is_valid(flag->env)) {
    free(spec.data);
    return 0;
  }
  if (!f2e_buffer_append(&spec, option) ||
      !f2e_buffer_append_char(&spec, '[') ||
      !f2e_buffer_append(&spec, flag->env[0] != '\0' ? flag->env : f2e_audit_flag_name(flag)) ||
      !f2e_buffer_append_char(&spec, ']')) {
    free(spec.data);
    return 0;
  }

  if (flag->type == F2E_TYPE_BOOL && !bool_negated) {
    if (!f2e_buffer_init(&values) ||
        !f2e_completion_add_bool_values(&values, flag) ||
        !f2e_buffer_append(&spec, "::value:(") ||
        !f2e_buffer_append(&spec, values.data) ||
        !f2e_buffer_append_char(&spec, ')')) {
      free(values.data);
      free(spec.data);
      return 0;
    }
    free(values.data);
  } else if (flag->type != F2E_TYPE_BOOL) {
    if (!f2e_buffer_append(&spec, ":value:")) {
      free(spec.data);
      return 0;
    }
  }

  int ok = f2e_completion_zsh_append_spec(script, &spec);
  free(spec.data);
  return ok;
}

static char *f2e_completion_script_zsh(const F2EConfig *config, const char *command_name) {
  F2EBuffer script;
  if (!f2e_buffer_init(&script)) {
    return NULL;
  }

  char command[F2E_MAX_NAME];
  if (!f2e_completion_command_name(command_name, command, sizeof(command))) {
    free(script.data);
    return NULL;
  }
  char function_name[F2E_MAX_NAME * 2];
  f2e_completion_function_name(command, function_name, sizeof(function_name));

  if (!f2e_buffer_append(&script, "#compdef ") ||
      !f2e_buffer_append(&script, command) ||
      !f2e_buffer_append_char(&script, '\n') ||
      !f2e_buffer_append(&script, function_name) ||
      !f2e_buffer_append(&script, "() {\n  _arguments -s \\\n")) {
    free(script.data);
    return NULL;
  }

  for (size_t i = 0; i < config->flag_count; i++) {
    const F2EFlag *flag = &config->flags[i];
    if (flag->env[0] == '\0') {
      continue;
    }
    for (size_t j = 0; j < flag->alias_count; j++) {
      if (!f2e_option_name_is_valid(flag->aliases[j])) {
        free(script.data);
        return NULL;
      }
      char option[F2E_MAX_NAME + 8];
      snprintf(option, sizeof(option), "--%s", flag->aliases[j]);
      if (!f2e_completion_zsh_option_spec(&script, option, flag, 0)) {
        free(script.data);
        return NULL;
      }
      if (flag->type == F2E_TYPE_BOOL) {
        snprintf(option, sizeof(option), "--no-%s", flag->aliases[j]);
        if (!f2e_completion_zsh_option_spec(&script, option, flag, 1)) {
          free(script.data);
          return NULL;
        }
      }
    }
    if (flag->short_name != '\0') {
      if (!isalnum((unsigned char)flag->short_name)) {
        free(script.data);
        return NULL;
      }
      char option[4] = {'-', flag->short_name, '\0', '\0'};
      if (!f2e_completion_zsh_option_spec(&script, option, flag, 0)) {
        free(script.data);
        return NULL;
      }
    }
  }

  if (!f2e_buffer_append(&script, "    '*::arg:->args'\n}\n") ||
      !f2e_buffer_append(&script, function_name) ||
      !f2e_buffer_append(&script, " \"$@\"\n")) {
    free(script.data);
    return NULL;
  }

  return script.data;
}

static char *f2e_completion_script_from_config(const F2EConfig *config, const char *shell, const char *command_name) {
  if (!config || !shell || shell[0] == '\0') {
    return NULL;
  }
  if (f2e_streq(shell, "bash")) {
    return f2e_completion_script_bash(config, command_name);
  }
  if (f2e_streq(shell, "zsh")) {
    return f2e_completion_script_zsh(config, command_name);
  }
  return NULL;
}

char *f2e_completion_script_from_file(const char *config_path, const char *shell, const char *command_name) {
  F2EConfig *config = (F2EConfig *)malloc(sizeof(F2EConfig));
  if (!config) {
    return NULL;
  }
  if (!config_path || !f2e_load_config(config_path, config)) {
    free(config);
    return NULL;
  }
  if (f2e_config_has_audit_errors(config)) {
    free(config);
    return NULL;
  }
  char *script = f2e_completion_script_from_config(config, shell, command_name);
  free(config);
  return script;
}

char *f2e_completion_script(const char *shell, const char *command_name) {
  char *path = f2e_default_config_path();
  if (!path) {
    return NULL;
  }
  char *script = f2e_completion_script_from_file(path, shell, command_name);
  free(path);
  return script;
}

typedef struct {
  char **items;
  size_t count;
} F2EHelpLines;

static size_t f2e_size_min(size_t a, size_t b) {
  return a < b ? a : b;
}

static size_t f2e_size_max(size_t a, size_t b) {
  return a > b ? a : b;
}

static int f2e_help_terminal_columns(void) {
  const char *env_columns = getenv("COLUMNS");
  if (env_columns && env_columns[0] != '\0') {
    char *end = NULL;
    errno = 0;
    long parsed = strtol(env_columns, &end, 10);
    if (errno == 0 && end && *end == '\0' && parsed > 0 && parsed <= 1000) {
      return (int)parsed;
    }
  }

#if defined(_WIN32)
  CONSOLE_SCREEN_BUFFER_INFO info;
  HANDLE out = GetStdHandle(STD_OUTPUT_HANDLE);
  if (out != INVALID_HANDLE_VALUE && GetConsoleScreenBufferInfo(out, &info)) {
    int columns = (int)(info.srWindow.Right - info.srWindow.Left + 1);
    if (columns > 0) {
      return columns;
    }
  }
#elif defined(TIOCGWINSZ) && (defined(__unix__) || defined(__APPLE__))
  struct winsize size;
  if (ioctl(STDOUT_FILENO, TIOCGWINSZ, &size) == 0 && size.ws_col > 0) {
    return (int)size.ws_col;
  }
#endif

  return 80;
}

static size_t f2e_help_resolve_columns(int terminal_columns) {
  int columns = terminal_columns > 0 ? terminal_columns : f2e_help_terminal_columns();
  if (columns < 40) {
    columns = 40;
  }
  if (columns > 160) {
    columns = 160;
  }
  return (size_t)columns;
}

static int f2e_help_append_repeat(F2EBuffer *buffer, char ch, size_t count) {
  for (size_t i = 0; i < count; i++) {
    if (!f2e_buffer_append_char(buffer, ch)) {
      return 0;
    }
  }
  return 1;
}

static int f2e_help_append_padded(F2EBuffer *buffer, const char *value, size_t width) {
  size_t len = value ? strlen(value) : 0;
  size_t used = f2e_size_min(len, width);
  if (used > 0) {
    if (!f2e_buffer_reserve(buffer, used)) {
      return 0;
    }
    memcpy(buffer->data + buffer->len, value, used);
    buffer->len += used;
    buffer->data[buffer->len] = '\0';
  }
  return f2e_help_append_repeat(buffer, ' ', width - used);
}

static int f2e_help_lines_push(F2EHelpLines *lines, const char *value, size_t len) {
  char *copy = (char *)malloc(len + 1);
  if (!copy) {
    return 0;
  }
  for (size_t i = 0; i < len; i++) {
    unsigned char ch = value ? (unsigned char)value[i] : '\0';
    if (ch == '\t') {
      copy[i] = ' ';
    } else if (ch < 0x20 || ch == 0x7f) {
      copy[i] = '?';
    } else {
      copy[i] = (char)ch;
    }
  }
  copy[len] = '\0';

  char **grown = (char **)realloc(lines->items, sizeof(char *) * (lines->count + 1));
  if (!grown) {
    free(copy);
    return 0;
  }
  lines->items = grown;
  lines->items[lines->count++] = copy;
  return 1;
}

static void f2e_help_lines_free(F2EHelpLines *lines) {
  if (!lines) {
    return;
  }
  for (size_t i = 0; i < lines->count; i++) {
    free(lines->items[i]);
  }
  free(lines->items);
  lines->items = NULL;
  lines->count = 0;
}

static int f2e_help_wrap_lines(const char *value, size_t width, F2EHelpLines *out) {
  memset(out, 0, sizeof(*out));
  if (width == 0) {
    width = 1;
  }

  const char *cursor = value ? value : "";
  if (*cursor == '\0') {
    return f2e_help_lines_push(out, "", 0);
  }

  while (*cursor) {
    while (*cursor == ' ' || *cursor == '\t') {
      cursor++;
    }
    if (*cursor == '\n' || *cursor == '\r') {
      if (!f2e_help_lines_push(out, "", 0)) {
        f2e_help_lines_free(out);
        return 0;
      }
      while (*cursor == '\n' || *cursor == '\r') {
        cursor++;
      }
      continue;
    }
    if (*cursor == '\0') {
      break;
    }

    size_t available = 0;
    while (cursor[available] && cursor[available] != '\n' && cursor[available] != '\r') {
      available++;
    }

    size_t take = f2e_size_min(available, width);
    if (available > width) {
      size_t break_at = 0;
      for (size_t i = 1; i < width; i++) {
        if (isspace((unsigned char)cursor[i])) {
          break_at = i;
        }
      }
      if (break_at > 0) {
        take = break_at;
      }
    }

    if (!f2e_help_lines_push(out, cursor, take)) {
      f2e_help_lines_free(out);
      return 0;
    }
    cursor += take;
    while (*cursor == ' ' || *cursor == '\t') {
      cursor++;
    }
    if (*cursor == '\n' || *cursor == '\r') {
      while (*cursor == '\n' || *cursor == '\r') {
        cursor++;
      }
    }
  }

  if (out->count == 0) {
    return f2e_help_lines_push(out, "", 0);
  }
  return 1;
}

static int f2e_help_append_border(F2EBuffer *buffer, const size_t *widths, size_t count) {
  if (!f2e_buffer_append_char(buffer, '+')) {
    return 0;
  }
  for (size_t i = 0; i < count; i++) {
    if (!f2e_help_append_repeat(buffer, '-', widths[i] + 2) ||
        !f2e_buffer_append_char(buffer, '+')) {
      return 0;
    }
  }
  return f2e_buffer_append_char(buffer, '\n');
}

static size_t f2e_help_table_width(const size_t *widths, size_t count) {
  size_t width = 1;
  for (size_t i = 0; i < count; i++) {
    width += widths[i] + 3;
  }
  return width;
}

static int f2e_help_append_spanning_row(F2EBuffer *buffer, const char *value, size_t table_width) {
  size_t inner_width = table_width > 4 ? table_width - 4 : 1;
  F2EHelpLines lines;
  if (!f2e_help_wrap_lines(value, inner_width, &lines)) {
    return 0;
  }
  int ok = 1;
  for (size_t i = 0; i < lines.count; i++) {
    if (!f2e_buffer_append(buffer, "| ") ||
        !f2e_help_append_padded(buffer, lines.items[i], inner_width) ||
        !f2e_buffer_append(buffer, " |\n")) {
      ok = 0;
      break;
    }
  }
  f2e_help_lines_free(&lines);
  return ok;
}

static int f2e_help_append_row(F2EBuffer *buffer, const char *const *cells, const size_t *widths, size_t count) {
  F2EHelpLines wrapped[5];
  if (count > 5) {
    return 0;
  }
  memset(wrapped, 0, sizeof(wrapped));

  size_t max_lines = 1;
  for (size_t i = 0; i < count; i++) {
    if (!f2e_help_wrap_lines(cells[i] ? cells[i] : "", widths[i], &wrapped[i])) {
      for (size_t j = 0; j <= i && j < count; j++) {
        f2e_help_lines_free(&wrapped[j]);
      }
      return 0;
    }
    max_lines = f2e_size_max(max_lines, wrapped[i].count);
  }

  int ok = 1;
  for (size_t line = 0; line < max_lines; line++) {
    if (!f2e_buffer_append_char(buffer, '|')) {
      ok = 0;
      break;
    }
    for (size_t col = 0; col < count; col++) {
      const char *cell = line < wrapped[col].count ? wrapped[col].items[line] : "";
      if (!f2e_buffer_append_char(buffer, ' ') ||
          !f2e_help_append_padded(buffer, cell, widths[col]) ||
          !f2e_buffer_append(buffer, " |")) {
        ok = 0;
        break;
      }
    }
    if (!ok || !f2e_buffer_append_char(buffer, '\n')) {
      ok = 0;
      break;
    }
  }

  for (size_t i = 0; i < count; i++) {
    f2e_help_lines_free(&wrapped[i]);
  }
  return ok;
}

static char *f2e_help_flag_names(const F2EFlag *flag) {
  F2EBuffer names;
  if (!f2e_buffer_init(&names)) {
    return NULL;
  }

  if (flag->short_name != '\0') {
    char short_name[3] = {'-', flag->short_name, '\0'};
    if (!f2e_buffer_append(&names, short_name)) {
      free(names.data);
      return NULL;
    }
  }

  for (size_t i = 0; i < flag->alias_count; i++) {
    if (names.len > 0 && !f2e_buffer_append(&names, ", ")) {
      free(names.data);
      return NULL;
    }
    if (!f2e_buffer_append(&names, "--") || !f2e_buffer_append(&names, flag->aliases[i])) {
      free(names.data);
      return NULL;
    }
  }

  if (names.len == 0 && flag->name[0] != '\0') {
    if (!f2e_buffer_append(&names, "--") || !f2e_buffer_append(&names, flag->name)) {
      free(names.data);
      return NULL;
    }
  }
  return names.data;
}

static int f2e_help_append_bool_values(F2EBuffer *buffer, const F2EFlag *flag) {
  if (!f2e_buffer_append(buffer, "true, false")) {
    return 0;
  }
  for (size_t i = 0; i < flag->true_alias_count; i++) {
    if (!f2e_buffer_append(buffer, ", ") || !f2e_buffer_append(buffer, flag->true_aliases[i])) {
      return 0;
    }
  }
  for (size_t i = 0; i < flag->false_alias_count; i++) {
    if (!f2e_buffer_append(buffer, ", ") || !f2e_buffer_append(buffer, flag->false_aliases[i])) {
      return 0;
    }
  }
  return 1;
}

static char *f2e_help_flag_description(const F2EFlag *flag) {
  F2EBuffer description;
  if (!f2e_buffer_init(&description)) {
    return NULL;
  }

  if (flag->help[0] != '\0' && !f2e_buffer_append(&description, flag->help)) {
    free(description.data);
    return NULL;
  }

  if (flag->type == F2E_TYPE_BOOL) {
    if (description.len > 0 && !f2e_buffer_append_char(&description, ' ')) {
      free(description.data);
      return NULL;
    }
    if (!f2e_buffer_append(&description, "Values: ") ||
        !f2e_help_append_bool_values(&description, flag) ||
        !f2e_buffer_append_char(&description, '.')) {
      free(description.data);
      return NULL;
    }
    if (flag->alias_count > 0) {
      if (!f2e_buffer_append(&description, " Negate with --no-") ||
          !f2e_buffer_append(&description, flag->aliases[0]) ||
          !f2e_buffer_append_char(&description, '.')) {
        free(description.data);
        return NULL;
      }
    }
  }

  if (description.len == 0 && !f2e_buffer_append_char(&description, '-')) {
    free(description.data);
    return NULL;
  }
  return description.data;
}

static int f2e_help_details_sep(F2EBuffer *details, int *wrote) {
  if (*wrote && !f2e_buffer_append(details, "; ")) {
    return 0;
  }
  *wrote = 1;
  return 1;
}

static char *f2e_help_flag_details_for_columns(const F2EFlag *flag, unsigned columns) {
  F2EBuffer details;
  if (!f2e_buffer_init(&details)) {
    return NULL;
  }

  int wrote = 0;
  if (columns & F2E_HELP_COL_ENV) {
    if (!f2e_help_details_sep(&details, &wrote) ||
        !f2e_buffer_append(&details, "env=") ||
        !f2e_buffer_append(&details, flag->env[0] != '\0' ? flag->env : "-")) {
      free(details.data);
      return NULL;
    }
  }

  if (columns & F2E_HELP_COL_TYPE) {
    if (!f2e_help_details_sep(&details, &wrote) ||
        !f2e_buffer_append(&details, "type=") ||
        !f2e_buffer_append(&details, f2e_value_type_name(flag->type))) {
      free(details.data);
      return NULL;
    }
  }

  if ((columns & F2E_HELP_COL_DEFAULT) && flag->has_default) {
    if (!f2e_help_details_sep(&details, &wrote) ||
        !f2e_buffer_append(&details, "default=") ||
        !f2e_buffer_append(&details, flag->default_value)) {
      free(details.data);
      return NULL;
    }
  }
  if ((columns & F2E_HELP_COL_DESCRIPTION) && flag->help[0] != '\0') {
    if (!f2e_help_details_sep(&details, &wrote) ||
        !f2e_buffer_append(&details, flag->help)) {
      free(details.data);
      return NULL;
    }
  }
  if ((columns & F2E_HELP_COL_DESCRIPTION) && flag->type == F2E_TYPE_BOOL) {
    if (!f2e_help_details_sep(&details, &wrote) ||
        !f2e_buffer_append(&details, "values=") ||
        !f2e_help_append_bool_values(&details, flag)) {
      free(details.data);
      return NULL;
    }
    if (flag->alias_count > 0) {
      if (!f2e_help_details_sep(&details, &wrote) ||
          !f2e_buffer_append(&details, "negates=--no-") ||
          !f2e_buffer_append(&details, flag->aliases[0])) {
        free(details.data);
        return NULL;
      }
    }
  }
  if (!wrote && !f2e_buffer_append_char(&details, '-')) {
    free(details.data);
    return NULL;
  }
  return details.data;
}

static void f2e_help_wide_widths(size_t terminal_columns, size_t widths[5]) {
  size_t columns = terminal_columns >= 110 ? terminal_columns : 110;
  size_t inner = columns - 16;
  widths[0] = columns >= 132 ? 32 : 27;
  widths[1] = columns >= 132 ? 20 : 16;
  widths[2] = columns >= 132 ? 10 : 9;
  widths[3] = columns >= 132 ? 14 : 12;
  size_t used = widths[0] + widths[1] + widths[2] + widths[3];
  widths[4] = inner > used ? inner - used : 24;
}

static void f2e_help_narrow_widths(size_t terminal_columns, size_t widths[2]) {
  size_t columns = terminal_columns >= 40 ? terminal_columns : 40;
  size_t inner = columns - 7;
  widths[0] = inner >= 64 ? 28 : inner >= 50 ? 22 : inner / 2;
  if (widths[0] < 14) {
    widths[0] = 14;
  }
  if (widths[0] + 16 > inner) {
    widths[0] = inner > 24 ? inner - 20 : inner / 2;
  }
  widths[1] = inner - widths[0];
}

static int f2e_help_command_name(const char *command_name, char *out, size_t out_size) {
  if (f2e_path_basename_copy(command_name, out, out_size)) {
    return 1;
  }
  return f2e_path_basename_copy("flags2env", out, out_size);
}

static unsigned f2e_help_selected_columns(const F2EConfig *config) {
  unsigned columns = config && config->help_columns_configured ? config->help_columns : F2E_HELP_COL_DEFAULTS;
  if (config) {
    columns &= ~config->help_exclude_columns;
  }
  columns &= F2E_HELP_COL_DEFAULTS;
  columns |= F2E_HELP_COL_OPTIONS;
  return columns;
}

static size_t f2e_help_collect_columns(unsigned columns, unsigned out[5]) {
  size_t count = 0;
  if (columns & F2E_HELP_COL_OPTIONS) {
    out[count++] = F2E_HELP_COL_OPTIONS;
  }
  if (columns & F2E_HELP_COL_ENV) {
    out[count++] = F2E_HELP_COL_ENV;
  }
  if (columns & F2E_HELP_COL_TYPE) {
    out[count++] = F2E_HELP_COL_TYPE;
  }
  if (columns & F2E_HELP_COL_DEFAULT) {
    out[count++] = F2E_HELP_COL_DEFAULT;
  }
  if (columns & F2E_HELP_COL_DESCRIPTION) {
    out[count++] = F2E_HELP_COL_DESCRIPTION;
  }
  return count;
}

static const char *f2e_help_column_header(unsigned column) {
  switch (column) {
    case F2E_HELP_COL_ENV:
      return "Env";
    case F2E_HELP_COL_TYPE:
      return "Type";
    case F2E_HELP_COL_DEFAULT:
      return "Default";
    case F2E_HELP_COL_DESCRIPTION:
      return "Description";
    case F2E_HELP_COL_OPTIONS:
    default:
      return "Option(s)";
  }
}

static int f2e_help_uses_default_wide_columns(const unsigned *columns, size_t column_count) {
  return column_count == 5 &&
         columns[0] == F2E_HELP_COL_OPTIONS &&
         columns[1] == F2E_HELP_COL_ENV &&
         columns[2] == F2E_HELP_COL_TYPE &&
         columns[3] == F2E_HELP_COL_DEFAULT &&
         columns[4] == F2E_HELP_COL_DESCRIPTION;
}

static size_t f2e_help_min_width_for_column(unsigned column) {
  switch (column) {
    case F2E_HELP_COL_ENV:
      return 10;
    case F2E_HELP_COL_TYPE:
      return 8;
    case F2E_HELP_COL_DEFAULT:
      return 10;
    case F2E_HELP_COL_DESCRIPTION:
      return 18;
    case F2E_HELP_COL_OPTIONS:
    default:
      return 18;
  }
}

static void f2e_help_custom_wide_widths(size_t terminal_columns,
                                        const unsigned *columns,
                                        size_t column_count,
                                        size_t widths[5]) {
  if (f2e_help_uses_default_wide_columns(columns, column_count)) {
    f2e_help_wide_widths(terminal_columns, widths);
    return;
  }

  size_t table_columns = terminal_columns >= 40 ? terminal_columns : 40;
  size_t separators = column_count * 3 + 1;
  size_t inner = table_columns > separators ? table_columns - separators : column_count * 12;
  size_t used = 0;
  size_t flexible_index = column_count > 0 ? column_count - 1 : 0;

  for (size_t i = 0; i < column_count; i++) {
    widths[i] = f2e_help_min_width_for_column(columns[i]);
    used += widths[i];
    if (columns[i] == F2E_HELP_COL_DESCRIPTION) {
      flexible_index = i;
    }
  }

  if (used < inner && column_count > 0) {
    widths[flexible_index] += inner - used;
  }
}

static const char *f2e_help_column_value(unsigned column,
                                         const F2EFlag *flag,
                                         char *names,
                                         char **description_out) {
  switch (column) {
    case F2E_HELP_COL_ENV:
      return flag->env[0] != '\0' ? flag->env : "-";
    case F2E_HELP_COL_TYPE:
      return f2e_value_type_name(flag->type);
    case F2E_HELP_COL_DEFAULT:
      return flag->has_default ? flag->default_value : "-";
    case F2E_HELP_COL_DESCRIPTION:
      if (!*description_out) {
        *description_out = f2e_help_flag_description(flag);
      }
      return *description_out;
    case F2E_HELP_COL_OPTIONS:
    default:
      return names;
  }
}

static char *f2e_help_table_from_config(const F2EConfig *config, const char *command_name, int terminal_columns) {
  if (!config) {
    return NULL;
  }

  size_t columns = f2e_help_resolve_columns(terminal_columns);
  int wide = columns >= 110;
  unsigned selected_columns = f2e_help_selected_columns(config);
  unsigned wide_columns[5] = {0};
  size_t widths[5];
  size_t column_count = 0;
  if (wide) {
    column_count = f2e_help_collect_columns(selected_columns, wide_columns);
    f2e_help_custom_wide_widths(columns, wide_columns, column_count, widths);
  } else if ((selected_columns & ~F2E_HELP_COL_OPTIONS) == 0) {
    widths[0] = columns > 4 ? columns - 4 : 36;
    column_count = 1;
  } else {
    f2e_help_narrow_widths(columns, widths);
    column_count = 2;
  }

  F2EBuffer table;
  if (!f2e_buffer_init(&table)) {
    return NULL;
  }

  char command[F2E_MAX_NAME];
  if (!f2e_help_command_name(command_name, command, sizeof(command))) {
    free(table.data);
    return NULL;
  }

  size_t table_width = f2e_help_table_width(widths, column_count);
  char title[F2E_MAX_VALUE];
  snprintf(title, sizeof(title), "Command: %s [OPTIONS]", command);
  if (!f2e_help_append_border(&table, widths, column_count) ||
      !f2e_help_append_spanning_row(&table, title, table_width) ||
      !f2e_help_append_border(&table, widths, column_count)) {
    free(table.data);
    return NULL;
  }

  if (wide) {
    const char *header[5] = {0};
    for (size_t i = 0; i < column_count; i++) {
      header[i] = f2e_help_column_header(wide_columns[i]);
    }
    if (!f2e_help_append_row(&table, header, widths, column_count) ||
        !f2e_help_append_border(&table, widths, column_count)) {
      free(table.data);
      return NULL;
    }
  } else if (column_count == 1) {
    const char *header[] = {"Option(s)"};
    if (!f2e_help_append_row(&table, header, widths, column_count) ||
        !f2e_help_append_border(&table, widths, column_count)) {
      free(table.data);
      return NULL;
    }
  } else {
    const char *header[] = {"Option(s)", "Details"};
    if (!f2e_help_append_row(&table, header, widths, column_count) ||
        !f2e_help_append_border(&table, widths, column_count)) {
      free(table.data);
      return NULL;
    }
  }

  for (size_t i = 0; i < config->flag_count; i++) {
    const F2EFlag *flag = &config->flags[i];
    char *names = f2e_help_flag_names(flag);
    if (!names) {
      free(table.data);
      return NULL;
    }

    int ok = 0;
    if (wide) {
      const char *row[5] = {0};
      char *description = NULL;
      for (size_t j = 0; j < column_count; j++) {
        row[j] = f2e_help_column_value(wide_columns[j], flag, names, &description);
      }
      if (!row[column_count - 1]) {
        free(description);
        free(names);
        free(table.data);
        return NULL;
      }
      ok = f2e_help_append_row(&table, row, widths, column_count);
      free(description);
    } else if (column_count == 1) {
      const char *row[] = {names};
      ok = f2e_help_append_row(&table, row, widths, column_count);
    } else {
      char *details = f2e_help_flag_details_for_columns(flag, selected_columns & ~F2E_HELP_COL_OPTIONS);
      if (!details) {
        free(names);
        free(table.data);
        return NULL;
      }
      const char *row[] = {names, details};
      ok = f2e_help_append_row(&table, row, widths, column_count);
      free(details);
    }
    free(names);

    if (!ok || !f2e_help_append_border(&table, widths, column_count)) {
      free(table.data);
      return NULL;
    }
  }

  if (config->help_url[0] != '\0') {
    char help_url[F2E_MAX_VALUE + 16];
    snprintf(help_url, sizeof(help_url), "More help: %s", config->help_url);
    if (!f2e_help_append_spanning_row(&table, help_url, table_width) ||
        !f2e_help_append_border(&table, widths, column_count)) {
      free(table.data);
      return NULL;
    }
  }

  return table.data;
}

static int f2e_print_stream_locked(FILE *stream, const char *value) {
  if (!stream || !value) {
    return 0;
  }
#if !defined(_WIN32)
  flockfile(stream);
#endif
  int ok = fputs(value, stream) != EOF;
  if (fflush(stream) == EOF) {
    ok = 0;
  }
#if !defined(_WIN32)
  funlockfile(stream);
#endif
  return ok;
}

int f2e_is_help_requested(int argc, const char *const argv[]) {
  if (argc < 0 || !argv) {
    return 0;
  }
  for (int i = 0; i < argc; i++) {
    if (argv[i] && f2e_streq(argv[i], "--")) {
      return 0;
    }
    if (argv[i] && f2e_streq(argv[i], "--help")) {
      return 1;
    }
  }
  return 0;
}

char *f2e_help_table_from_file(const char *config_path, const char *command_name, int terminal_columns) {
  F2EConfig *config = (F2EConfig *)malloc(sizeof(F2EConfig));
  if (!config) {
    return NULL;
  }
  if (!config_path || !f2e_load_config(config_path, config)) {
    free(config);
    return NULL;
  }
  char *table = f2e_help_table_from_config(config, command_name, terminal_columns);
  free(config);
  return table;
}

char *f2e_help_table(const char *command_name, int terminal_columns) {
  char *path = f2e_default_config_path();
  if (!path) {
    return NULL;
  }
  char *table = f2e_help_table_from_file(path, command_name, terminal_columns);
  free(path);
  return table;
}

int f2e_print_table_from_file(const char *config_path, const char *command_name, int terminal_columns) {
  char *table = f2e_help_table_from_file(config_path, command_name, terminal_columns);
  if (!table) {
    return 1;
  }
  int ok = f2e_print_stream_locked(stdout, table);
  f2e_free(table);
  return ok ? 0 : 1;
}

int f2e_print_table(const char *command_name, int terminal_columns) {
  char *table = f2e_help_table(command_name, terminal_columns);
  if (!table) {
    return 1;
  }
  int ok = f2e_print_stream_locked(stdout, table);
  f2e_free(table);
  return ok ? 0 : 1;
}

char *f2e_parse_from_file(const char *config_path, int argc, const char *const argv[]) {
  if (argc < 0 || !argv) {
    argc = 0;
  }

  F2EConfig *config = (F2EConfig *)malloc(sizeof(F2EConfig));
  if (!config) {
    return f2e_empty_json_object();
  }

  if (!config_path || !f2e_load_config(config_path, config)) {
    free(config);
    return f2e_empty_json_object();
  }

  F2EPair *pairs = (F2EPair *)calloc(F2E_MAX_PAIRS, sizeof(F2EPair));
  if (!pairs) {
    free(config);
    return f2e_empty_json_object();
  }

  F2EJsonList positionals = {0};
  F2EJsonList unknown_options = {0};
  F2EJsonList errors = {0};
  int track_positionals = config->positionals_env[0] != '\0' && f2e_json_list_init(&positionals);
  int track_unknown_options = config->unknown_options_env[0] != '\0' && f2e_json_list_init(&unknown_options);
  int track_errors = config->errors_env[0] != '\0' && f2e_json_list_init(&errors);
  int allow_unknown = f2e_resolve_allow_unknown(config, argc, argv);

  f2e_apply_defaults(config, pairs, F2E_MAX_PAIRS);

  for (int i = 0; i < argc; i++) {
    const char *token = argv[i];
    if (!token || token[0] != '-' || token[1] == '\0') {
      if (track_positionals) {
        if (config->stop_at_first_positional) {
          for (int j = i; j < argc; j++) {
            f2e_json_list_append(&positionals, argv[j]);
          }
          break;
        }
        f2e_json_list_append(&positionals, token);
      } else if (config->stop_at_first_positional) {
        break;
      }
      continue;
    }
    if (strcmp(token, "--") == 0) {
      if (track_positionals) {
        for (int j = i + 1; j < argc; j++) {
          f2e_json_list_append(&positionals, argv[j]);
        }
      }
      break;
    }
    if (!f2e_token_looks_like_known_option(config, token)) {
      int parsed_allow_unknown = 0;
      if (f2e_token_sets_allow_unknown(token, &parsed_allow_unknown)) {
        allow_unknown = parsed_allow_unknown;
      } else if (!allow_unknown && track_unknown_options) {
        f2e_json_list_append(&unknown_options, token);
      }
      continue;
    }
    if (token[1] == '-') {
      f2e_apply_long_arg(config, pairs, F2E_MAX_PAIRS, token, &i, argc, argv, track_errors ? &errors : NULL);
    } else {
      f2e_apply_short_arg(config, pairs, F2E_MAX_PAIRS, token, &i, argc, argv, track_errors ? &errors : NULL);
    }
  }

  if (track_positionals && positionals.count > 0) {
    char value[F2E_MAX_VALUE];
    if (f2e_json_list_finish(&positionals, value, sizeof(value))) {
      f2e_set_pair(pairs, F2E_MAX_PAIRS, config->positionals_env, value);
    }
  }
  if (track_unknown_options && unknown_options.count > 0) {
    char value[F2E_MAX_VALUE];
    if (f2e_json_list_finish(&unknown_options, value, sizeof(value))) {
      f2e_set_pair(pairs, F2E_MAX_PAIRS, config->unknown_options_env, value);
    }
  }
  if (track_errors && errors.count > 0) {
    char value[F2E_MAX_VALUE];
    if (f2e_json_list_finish(&errors, value, sizeof(value))) {
      f2e_set_pair(pairs, F2E_MAX_PAIRS, config->errors_env, value);
    }
  }

  f2e_json_list_discard(&positionals);
  f2e_json_list_discard(&unknown_options);
  f2e_json_list_discard(&errors);

  char *json = f2e_pairs_to_json(pairs, F2E_MAX_PAIRS);
  free(pairs);
  free(config);
  if (!json) {
    json = f2e_empty_json_object();
  }
  return json;
}

char *f2e_parse(int argc, const char *const argv[]) {
  char *path = f2e_default_config_path();
  if (!path) {
    return f2e_empty_json_object();
  }
  char *result = f2e_parse_from_file(path, argc, argv);
  free(path);
  return result;
}

static int f2e_json_array_append(char ***items, int *count, int *cap, const char *value) {
  if (*count >= *cap) {
    int next = *cap == 0 ? 8 : *cap * 2;
    char **grown = (char **)realloc(*items, sizeof(char *) * (size_t)next);
    if (!grown) {
      return 0;
    }
    *items = grown;
    *cap = next;
  }
  char *copy = (char *)malloc(strlen(value) + 1);
  if (!copy) {
    return 0;
  }
  strcpy(copy, value);
  (*items)[(*count)++] = copy;
  return 1;
}

static int f2e_parse_json_string_token(const char **cursor_ref, char *out, size_t out_size) {
  const char *cursor = f2e_trim_left((char *)*cursor_ref);
  size_t len = 0;
  if (*cursor != '"') {
    return 0;
  }
  cursor++;
  while (*cursor && *cursor != '"') {
    char ch = *cursor++;
    if (ch == '\\' && *cursor) {
      char escaped = *cursor++;
      switch (escaped) {
        case 'b':
          ch = '\b';
          break;
        case 'f':
          ch = '\f';
          break;
        case 'n':
          ch = '\n';
          break;
        case 'r':
          ch = '\r';
          break;
        case 't':
          ch = '\t';
          break;
        case '"':
        case '\\':
        case '/':
          ch = escaped;
          break;
        case 'u':
          if (cursor[0] && cursor[1] && cursor[2] && cursor[3]) {
            ch = '?';
            cursor += 4;
          }
          break;
        default:
          ch = escaped;
          break;
      }
    }
    if (len + 1 < out_size) {
      out[len++] = ch;
    }
  }
  if (*cursor != '"') {
    return 0;
  }
  cursor++;
  if (out_size > 0) {
    out[len] = '\0';
  }
  *cursor_ref = cursor;
  return 1;
}

static void f2e_free_json_items(char **items, int count);

static int f2e_parse_json_argv_items(const char *argv_json, char ***items, int *count) {
  const char *cursor = f2e_trim_left((char *)argv_json);
  int cap = 0;
  int expecting_value = 1;
  int saw_value = 0;
  *items = NULL;
  *count = 0;

  if (!argv_json || *cursor != '[') {
    return 0;
  }
  cursor++;

  while (*cursor) {
    cursor = f2e_trim_left((char *)cursor);
    if (*cursor == ']') {
      return !saw_value || !expecting_value;
    }

    char value[F2E_MAX_VALUE];
    if (!f2e_parse_json_string_token(&cursor, value, sizeof(value))) {
      return 0;
    }
    if (!f2e_json_array_append(items, count, &cap, value)) {
      return 0;
    }
    saw_value = 1;
    expecting_value = 0;

    cursor = f2e_trim_left((char *)cursor);
    if (*cursor == ',') {
      cursor++;
      expecting_value = 1;
      continue;
    }
    if (*cursor == ']') {
      return 1;
    }
    return 0;
  }
  return 0;
}

int f2e_is_help_requested_json_argv(const char *argv_json) {
  char **items = NULL;
  int count = 0;
  if (!argv_json || !f2e_parse_json_argv_items(argv_json, &items, &count)) {
    f2e_free_json_items(items, count);
    return 0;
  }
  int requested = f2e_is_help_requested(count, (const char *const *)items);
  f2e_free_json_items(items, count);
  return requested;
}

static void f2e_free_json_items(char **items, int count) {
  if (!items) {
    return;
  }
  for (int i = 0; i < count; i++) {
    free(items[i]);
  }
  free(items);
}

static char *f2e_empty_json_object(void) {
  char *empty = (char *)malloc(3);
  if (empty) {
    f2e_strlcpy(empty, "{}", 3);
  }
  return empty;
}

#if defined(__linux__)
static int f2e_read_process_argv(char ***items, int *count) {
  FILE *file = fopen("/proc/self/cmdline", "rb");
  if (!file) {
    return 0;
  }

  size_t len = 0;
  size_t cap = 256;
  char *data = (char *)malloc(cap);
  if (!data) {
    fclose(file);
    return 0;
  }

  int ch;
  while ((ch = fgetc(file)) != EOF) {
    if (len + 1 >= cap) {
      cap *= 2;
      char *grown = (char *)realloc(data, cap);
      if (!grown) {
        free(data);
        fclose(file);
        return 0;
      }
      data = grown;
    }
    data[len++] = (char)ch;
  }
  fclose(file);

  int argv_cap = 0;
  *items = NULL;
  *count = 0;
  size_t start = 0;
  for (size_t i = 0; i <= len; i++) {
    if (i == len || data[i] == '\0') {
      if (i > start && !f2e_json_array_append(items, count, &argv_cap, data + start)) {
        f2e_free_json_items(*items, *count);
        *items = NULL;
        *count = 0;
        free(data);
        return 0;
      }
      start = i + 1;
    }
  }

  free(data);
  return *count > 0;
}
#elif defined(__APPLE__)
static int f2e_read_process_argv(char ***items, int *count) {
  int mib[3] = {CTL_KERN, KERN_PROCARGS2, getpid()};
  size_t size = 0;
  if (sysctl(mib, 3, NULL, &size, NULL, 0) != 0 || size == 0) {
    return 0;
  }

  char *data = (char *)malloc(size);
  if (!data) {
    return 0;
  }
  if (sysctl(mib, 3, data, &size, NULL, 0) != 0) {
    free(data);
    return 0;
  }

  int argc = 0;
  memcpy(&argc, data, sizeof(argc));
  if (argc <= 0) {
    free(data);
    return 0;
  }

  char *cursor = data + sizeof(argc);
  char *end = data + size;
  while (cursor < end && *cursor != '\0') {
    cursor++;
  }
  while (cursor < end && *cursor == '\0') {
    cursor++;
  }

  int argv_cap = 0;
  *items = NULL;
  *count = 0;
  for (int i = 0; i < argc && cursor < end; i++) {
    if (!f2e_json_array_append(items, count, &argv_cap, cursor)) {
      f2e_free_json_items(*items, *count);
      *items = NULL;
      *count = 0;
      free(data);
      return 0;
    }
    while (cursor < end && *cursor != '\0') {
      cursor++;
    }
    while (cursor < end && *cursor == '\0') {
      cursor++;
    }
  }

  free(data);
  return *count > 0;
}
#elif defined(_WIN32)
static int f2e_read_process_argv(char ***items, int *count) {
  int argc = 0;
  LPWSTR *wide_argv = CommandLineToArgvW(GetCommandLineW(), &argc);
  if (!wide_argv || argc <= 0) {
    return 0;
  }

  int argv_cap = 0;
  *items = NULL;
  *count = 0;
  for (int i = 0; i < argc; i++) {
    int utf8_len = WideCharToMultiByte(CP_UTF8, 0, wide_argv[i], -1, NULL, 0, NULL, NULL);
    if (utf8_len <= 0) {
      continue;
    }
    char *value = (char *)malloc((size_t)utf8_len);
    if (!value) {
      f2e_free_json_items(*items, *count);
      LocalFree(wide_argv);
      return 0;
    }
    WideCharToMultiByte(CP_UTF8, 0, wide_argv[i], -1, value, utf8_len, NULL, NULL);
    if (!f2e_json_array_append(items, count, &argv_cap, value)) {
      free(value);
      f2e_free_json_items(*items, *count);
      LocalFree(wide_argv);
      return 0;
    }
    free(value);
  }

  LocalFree(wide_argv);
  return *count > 0;
}
#else
static int f2e_read_process_argv(char ***items, int *count) {
  *items = NULL;
  *count = 0;
  return 0;
}
#endif

char *f2e_parse_process_from_file(const char *config_path) {
  char **items = NULL;
  int count = 0;
  if (!f2e_read_process_argv(&items, &count)) {
    return f2e_parse_from_file(config_path, 0, NULL);
  }

  char *result = f2e_parse_from_file(config_path, count, (const char *const *)items);
  f2e_free_json_items(items, count);
  return result;
}

char *f2e_parse_process(void) {
  char *path = f2e_default_config_path();
  if (!path) {
    return f2e_empty_json_object();
  }
  char *result = f2e_parse_process_from_file(path);
  free(path);
  return result;
}

char *f2e_parse_json_argv_from_file(const char *config_path, const char *argv_json) {
  char **items = NULL;
  int count = 0;
  if (!argv_json || !f2e_parse_json_argv_items(argv_json, &items, &count)) {
    f2e_free_json_items(items, count);
    return f2e_empty_json_object();
  }

  char *result = f2e_parse_from_file(config_path, count, (const char *const *)items);
  f2e_free_json_items(items, count);
  return result;
}

char *f2e_parse_json_argv(const char *argv_json) {
  char *path = f2e_default_config_path();
  if (!path) {
    return f2e_empty_json_object();
  }
  char *result = f2e_parse_json_argv_from_file(path, argv_json);
  free(path);
  return result;
}

void f2e_free(char *value) {
  free(value);
}
