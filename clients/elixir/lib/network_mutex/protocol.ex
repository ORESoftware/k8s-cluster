defmodule NetworkMutex.Protocol do
  @moduledoc """
  Protocol mirror for the dd-rust-network-mutex broker.

  The broker uses newline-delimited JSON with camelCase fields. This module is
  intentionally dependency-free; it provides typed discriminator atoms and
  compact request builders that match `src/protocol.rs`.
  """

  @protocol_version "0.1.0"
  @max_composite_keys 5

  @request_types ~w(version auth lock unlock registerRead registerWrite endRead endWrite lockInfo ls heartbeat)
  @response_types ~w(version auth lock compositeLock unlock registerReadResult registerWriteResult endReadResult endWriteResult lockInfo lsResult reelection error ok)

  def protocol_version, do: @protocol_version
  def max_composite_keys, do: @max_composite_keys
  def request_types, do: @request_types
  def response_types, do: @response_types

  def request_type_to_wire(:version), do: "version"
  def request_type_to_wire(:auth), do: "auth"
  def request_type_to_wire(:lock), do: "lock"
  def request_type_to_wire(:unlock), do: "unlock"
  def request_type_to_wire(:register_read), do: "registerRead"
  def request_type_to_wire(:register_write), do: "registerWrite"
  def request_type_to_wire(:end_read), do: "endRead"
  def request_type_to_wire(:end_write), do: "endWrite"
  def request_type_to_wire(:lock_info), do: "lockInfo"
  def request_type_to_wire(:ls), do: "ls"
  def request_type_to_wire(:heartbeat), do: "heartbeat"

  def response_type_from_wire("version"), do: :version
  def response_type_from_wire("auth"), do: :auth
  def response_type_from_wire("lock"), do: :lock
  def response_type_from_wire("compositeLock"), do: :composite_lock
  def response_type_from_wire("unlock"), do: :unlock
  def response_type_from_wire("registerReadResult"), do: :register_read_result
  def response_type_from_wire("registerWriteResult"), do: :register_write_result
  def response_type_from_wire("endReadResult"), do: :end_read_result
  def response_type_from_wire("endWriteResult"), do: :end_write_result
  def response_type_from_wire("lockInfo"), do: :lock_info
  def response_type_from_wire("lsResult"), do: :ls_result
  def response_type_from_wire("reelection"), do: :reelection
  def response_type_from_wire("error"), do: :error
  def response_type_from_wire("ok"), do: :ok
  def response_type_from_wire(_), do: :unknown

  def version_request(uuid, value \\ @protocol_version) do
    frame(type: "version", uuid: uuid, value: value)
  end

  def auth_request(uuid, token) do
    frame(type: "auth", uuid: uuid, token: token)
  end

  def lock_request_single(uuid, key, opts \\ []) do
    frame(
      compact(
        type: "lock",
        uuid: uuid,
        key: key,
        ttl: positive_or_nil(Keyword.get(opts, :ttl_ms, 0)),
        max: Keyword.get(opts, :max_holders),
        wait: Keyword.get(opts, :wait)
      )
    )
  end

  def lock_request_composite(uuid, keys, opts \\ []) do
    count = length(keys)

    if count < 1 or count > @max_composite_keys do
      raise ArgumentError, "composite key count must be 1..=5, got #{count}"
    end

    frame(
      compact(
        type: "lock",
        uuid: uuid,
        keys: keys,
        ttl: positive_or_nil(Keyword.get(opts, :ttl_ms, 0)),
        wait: Keyword.get(opts, :wait)
      )
    )
  end

  def unlock_request_single(uuid, key, lock_uuid, force \\ false) do
    frame(compact(type: "unlock", uuid: uuid, key: key, lockUuid: blank_or_nil(lock_uuid), force: true_or_nil(force)))
  end

  def unlock_request_composite(uuid, keys, lock_uuid) do
    frame(compact(type: "unlock", uuid: uuid, keys: keys, lockUuid: blank_or_nil(lock_uuid)))
  end

  def rw_request(type, uuid, key), do: frame(type: request_type_to_wire(type), uuid: uuid, key: key)
  def lock_info_request(uuid, key), do: rw_request(:lock_info, uuid, key)
  def ls_request(uuid), do: frame(type: "ls", uuid: uuid)
  def heartbeat_request(uuid), do: frame(type: "heartbeat", uuid: uuid)

  defp positive_or_nil(n) when is_integer(n) and n > 0, do: n
  defp positive_or_nil(_), do: nil

  defp true_or_nil(true), do: true
  defp true_or_nil(_), do: nil

  defp blank_or_nil(nil), do: nil
  defp blank_or_nil(""), do: nil
  defp blank_or_nil(v), do: v

  defp compact(fields) do
    Enum.reject(fields, fn {_k, v} -> is_nil(v) end)
  end

  defp frame(fields) do
    "{" <> Enum.map_join(fields, ",", &field_to_json/1) <> "}\n"
  end

  defp field_to_json({key, value}) when is_atom(key), do: field_to_json({Atom.to_string(key), value})
  defp field_to_json({key, value}) when is_binary(value), do: quote(key) <> ":" <> quote(value)
  defp field_to_json({key, value}) when is_integer(value), do: quote(key) <> ":" <> Integer.to_string(value)
  defp field_to_json({key, true}), do: quote(key) <> ":true"
  defp field_to_json({key, false}), do: quote(key) <> ":false"
  defp field_to_json({key, values}) when is_list(values), do: quote(key) <> ":[" <> Enum.map_join(values, ",", &quote/1) <> "]"

  defp quote(value) do
    "\"" <> escape(to_string(value)) <> "\""
  end

  defp escape(value) do
    value
    |> String.replace("\\", "\\\\")
    |> String.replace("\"", "\\\"")
    |> String.replace("\n", "\\n")
  end
end

