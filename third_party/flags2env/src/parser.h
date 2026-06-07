#ifndef F2E_PARSER_H
#define F2E_PARSER_H

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

#define F2E_VERSION "0.1.0"

#if defined(__clang__) || defined(__GNUC__)
#define F2E_WARN_UNUSED_RESULT __attribute__((warn_unused_result))
#else
#define F2E_WARN_UNUSED_RESULT
#endif

#ifndef __has_attribute
#define __has_attribute(attribute_name) 0
#endif

#if defined(__clang__) && __has_attribute(ownership_returns) && __has_attribute(ownership_takes)
#define F2E_OWNED_RESULT __attribute__((ownership_returns(malloc))) F2E_WARN_UNUSED_RESULT
#define F2E_TAKES_OWNED_ARG_1 __attribute__((ownership_takes(malloc, 1)))
#else
#define F2E_OWNED_RESULT F2E_WARN_UNUSED_RESULT
#define F2E_TAKES_OWNED_ARG_1
#endif

const char *f2e_version(void);

/*
 * Parses argv using the nearest .cli-flags.toml found by walking upward from
 * the current working directory. Refuses to use $HOME/.cli-flags.toml. Returns
 * a heap-allocated JSON object string. Call f2e_free() with the returned pointer.
 */
char *f2e_parse(int argc, const char *const argv[]) F2E_OWNED_RESULT;

/*
 * Parses argv using an explicit TOML config path and returns a heap-allocated
 * JSON object string. Call f2e_free() with the returned pointer.
 */
char *f2e_parse_from_file(const char *config_path, int argc, const char *const argv[]) F2E_OWNED_RESULT;

/*
 * Parses the current process command line using the nearest .cli-flags.toml
 * where the host OS exposes process argv. Explicit f2e_parse(...) is still
 * preferred when the caller has already adjusted, sliced, or synthesized argv.
 */
char *f2e_parse_process(void) F2E_OWNED_RESULT;

/*
 * Parses the current process command line using an explicit TOML config path.
 */
char *f2e_parse_process_from_file(const char *config_path) F2E_OWNED_RESULT;

/*
 * FFI-friendly entrypoint: argv_json must be a JSON array of strings.
 * Returns a heap-allocated JSON object string. Call f2e_free().
 */
char *f2e_parse_json_argv(const char *argv_json) F2E_OWNED_RESULT;

/*
 * FFI-friendly entrypoint with an explicit config path.
 */
char *f2e_parse_json_argv_from_file(const char *config_path, const char *argv_json) F2E_OWNED_RESULT;

/*
 * Detects the exact --help token without consuming or parsing other flags.
 * Language clients can use this to expose lazy help-menu behavior.
 */
int f2e_is_help_requested(int argc, const char *const argv[]) F2E_WARN_UNUSED_RESULT;
int f2e_is_help_requested_json_argv(const char *argv_json) F2E_WARN_UNUSED_RESULT;

/*
 * Generates and prints a terminal-width-aware help table from .cli-flags.toml.
 * Pass terminal_columns <= 0 to auto-detect from $COLUMNS or the active
 * terminal. The returned table is heap-allocated; call f2e_free().
 */
char *f2e_help_table(const char *command_name, int terminal_columns) F2E_OWNED_RESULT;
char *f2e_help_table_from_file(const char *config_path, const char *command_name, int terminal_columns) F2E_OWNED_RESULT;
int f2e_print_table(const char *command_name, int terminal_columns) F2E_WARN_UNUSED_RESULT;
int f2e_print_table_from_file(const char *config_path, const char *command_name, int terminal_columns) F2E_WARN_UNUSED_RESULT;

/*
 * Audits .cli-flags.toml for parse issues, ambiguous aliases, duplicate short
 * flags, env collisions, and boolean value alias conflicts. Returns a
 * heap-allocated JSON report. Call f2e_free().
 */
char *f2e_audit_config(void) F2E_OWNED_RESULT;
char *f2e_audit_config_from_file(const char *config_path) F2E_OWNED_RESULT;
int f2e_audit_config_status(void) F2E_WARN_UNUSED_RESULT;
int f2e_audit_config_status_from_file(const char *config_path) F2E_WARN_UNUSED_RESULT;

/*
 * Generates static shell completion scripts from .cli-flags.toml. The generated
 * scripts are optimized for shell startup/completion speed: they do not invoke
 * flags2env or read TOML at completion time.
 */
char *f2e_completion_script(const char *shell, const char *command_name) F2E_OWNED_RESULT;
char *f2e_completion_script_from_file(const char *config_path, const char *shell, const char *command_name) F2E_OWNED_RESULT;

/*
 * Audits a .env file against the env keys declared by .cli-flags.toml.
 * Unknown .env keys are errors unless ignored by config; declared TOML env
 * keys missing from .env are warnings because they may be optional or supplied
 * elsewhere.
 */
char *f2e_audit_env_file(void) F2E_OWNED_RESULT;
char *f2e_audit_env_file_from_file(const char *config_path, const char *env_path) F2E_OWNED_RESULT;
int f2e_audit_env_file_status(void) F2E_WARN_UNUSED_RESULT;
int f2e_audit_env_file_status_from_file(const char *config_path, const char *env_path) F2E_WARN_UNUSED_RESULT;

void f2e_free(char *value) F2E_TAKES_OWNED_ARG_1;

#ifdef __cplusplus
}
#endif

#endif
