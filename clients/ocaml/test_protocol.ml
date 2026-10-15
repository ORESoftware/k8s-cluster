let contains s sub =
  let len = String.length s and sub_len = String.length sub in
  let rec loop i =
    i + sub_len <= len && (String.sub s i sub_len = sub || loop (i + 1))
  in
  sub_len = 0 || loop 0

let check cond name =
  if not cond then (
    prerr_endline ("FAIL: " ^ name);
    exit 1)

let () =
  let frame =
    Network_mutex_protocol.lock_request_single ~ttl:4000 ~max:1 ~wait:false "u-1" "k1"
  in
  check (contains frame "\"type\":\"lock\"") "lock type";
  check (contains frame "\"key\":\"k1\"") "key field";
  check (contains frame "\"ttl\":4000") "ttl field";
  check (contains frame "\"max\":1") "max field";
  check (contains frame "\"wait\":false") "wait false";
  check (String.ends_with ~suffix:"\n" frame) "newline";

  let composite =
    Network_mutex_protocol.lock_request_composite ~wait:true "u-2" [ "c"; "a"; "b" ]
  in
  check (contains composite "\"keys\":[\"c\",\"a\",\"b\"]") "composite keys";
  check (contains composite "\"wait\":true") "wait true";

  let oversize_rejected =
    try
      ignore
        (Network_mutex_protocol.lock_request_composite "u" [ "a"; "b"; "c"; "d"; "e"; "f" ]);
      false
    with Invalid_argument _ -> true
  in
  check oversize_rejected "oversize composite rejected";

  check
    (Network_mutex_protocol.response_type_from_wire "compositeLock"
    = Network_mutex_protocol.CompositeLockResponse)
    "composite response type";
  check
    (Network_mutex_protocol.response_type_from_wire "registerReadResult"
    = Network_mutex_protocol.RegisterReadResult)
    "register read result";
  check (Network_mutex_protocol.response_type_from_wire "ok" = Network_mutex_protocol.OkResponse) "ok";
  check
    (Network_mutex_protocol.response_type_from_wire "bogus"
    = Network_mutex_protocol.UnknownResponse)
    "unknown response";

  print_endline "[test-ocaml] all protocol tests passed"

