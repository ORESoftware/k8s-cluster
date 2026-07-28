//// Executable HTTP route contract for the presence service.
////
//// This registry is the single source of truth for:
////
////   * runtime request matching and dispatch;
////   * stable operation IDs and visibility/auth metadata;
////   * the internal OpenAPI 3.1 document;
////   * the fail-closed public OpenAPI projection consumed by generated SDKs.
////
//// Do not add a route directly to `http_server.gleam`. Add it here and
//// dispatch its `RouteId` in the server. Contract tests reject duplicate
//// operation IDs and prove that every registered route is matchable.

import gleam/http.{type Method, Delete, Get, Post}
import gleam/json
import gleam/list

pub type RouteId {
  Help
  PublicOpenApi
  PublicDocs
  Health
  Nodes
  WebSocket
  AddConversationMember
  RemoveConversationMember
  ListConversationMembers
  BroadcastConversation
  BroadcastUser
  LogoutDevice
  RuntimeConfigSnapshot
  RuntimeConfigApply
  RuntimeConfigReset
}

pub type Segment {
  Literal(String)
  Variable(String)
}

pub type Visibility {
  Public
  Internal
}

pub type AuthBoundary {
  Anonymous
  ClusterNetworkPolicy
  RuntimeConfigServerSecret
}

pub type ParameterSpec {
  ParameterSpec(name: String, required: Bool, description: String)
}

pub type RequestBodySpec {
  NoRequestBody
  RequestBody(
    required: Bool,
    description: String,
    content_type: String,
    schema: Schema,
  )
}

pub type Schema {
  EmptySchema
  StringSchema
  IntegerSchema
  BooleanSchema
  FreeFormSchema
  ArraySchema(Schema)
  ObjectSchema(List(#(String, Schema, Bool)))
}

pub type ResponseSpec {
  ResponseSpec(
    status: String,
    description: String,
    content_type: String,
    schema: Schema,
  )
}

pub type Protocol {
  Http
  WebSocketProtocol
}

pub type RouteSpec {
  RouteSpec(
    id: RouteId,
    method: Method,
    path: String,
    segments: List(Segment),
    operation_id: String,
    summary: String,
    description: String,
    tag: String,
    route_type: String,
    visibility: Visibility,
    auth: AuthBoundary,
    query_parameters: List(ParameterSpec),
    request_body: RequestBodySpec,
    responses: List(ResponseSpec),
    protocol: Protocol,
  )
}

pub type RouteMatch {
  RouteMatch(route: RouteSpec, parameters: List(#(String, String)))
}

pub fn routes() -> List(RouteSpec) {
  [
    route(
      id: Help,
      method: Get,
      path: "/",
      segments: [],
      operation_id: "getPresenceHelp",
      summary: "Presence service help",
      description: "Returns a concise plain-text overview of the service routes.",
      tag: "service",
      route_type: "service",
      visibility: Public,
      auth: Anonymous,
      query_parameters: [],
      request_body: NoRequestBody,
      responses: [text_response("200", "Presence service help text.")],
      protocol: Http,
    ),
    route(
      id: PublicOpenApi,
      method: Get,
      path: "/openapi.json",
      segments: [Literal("openapi.json")],
      operation_id: "getPresenceOpenApi",
      summary: "Public OpenAPI contract",
      description: "Returns the fail-closed public OpenAPI 3.1 contract generated from this route registry.",
      tag: "documentation",
      route_type: "service",
      visibility: Public,
      auth: Anonymous,
      query_parameters: [],
      request_body: NoRequestBody,
      responses: [
        json_response("200", "Public OpenAPI 3.1 document.", FreeFormSchema),
      ],
      protocol: Http,
    ),
    route(
      id: PublicOpenApi,
      method: Get,
      path: "/api/docs.json",
      segments: [Literal("api"), Literal("docs.json")],
      operation_id: "getPresenceOpenApiAlias",
      summary: "Public OpenAPI contract alias",
      description: "Compatibility alias for /openapi.json.",
      tag: "documentation",
      route_type: "service",
      visibility: Public,
      auth: Anonymous,
      query_parameters: [],
      request_body: NoRequestBody,
      responses: [
        json_response("200", "Public OpenAPI 3.1 document.", FreeFormSchema),
      ],
      protocol: Http,
    ),
    route(
      id: PublicDocs,
      method: Get,
      path: "/api/docs",
      segments: [Literal("api"), Literal("docs")],
      operation_id: "getPresenceApiReference",
      summary: "Interactive API reference",
      description: "Returns the Scalar API reference backed by /openapi.json.",
      tag: "documentation",
      route_type: "service",
      visibility: Public,
      auth: Anonymous,
      query_parameters: [],
      request_body: NoRequestBody,
      responses: [html_response("200", "Interactive API documentation.")],
      protocol: Http,
    ),
    route(
      id: PublicDocs,
      method: Get,
      path: "/docs/api",
      segments: [Literal("docs"), Literal("api")],
      operation_id: "getPresenceApiReferenceAlias",
      summary: "Interactive API reference alias",
      description: "Compatibility alias for /api/docs.",
      tag: "documentation",
      route_type: "service",
      visibility: Public,
      auth: Anonymous,
      query_parameters: [],
      request_body: NoRequestBody,
      responses: [html_response("200", "Interactive API documentation.")],
      protocol: Http,
    ),
    route(
      id: Health,
      method: Get,
      path: "/healthz",
      segments: [Literal("healthz")],
      operation_id: "getPresenceHealth",
      summary: "Presence process health",
      description: "Reports process health and cluster-local topology details. This operation is intentionally absent from the public contract.",
      tag: "operations",
      route_type: "service",
      visibility: Internal,
      auth: ClusterNetworkPolicy,
      query_parameters: [],
      request_body: NoRequestBody,
      responses: [
        json_response(
          "200",
          "Healthy presence process.",
          ObjectSchema([
            #("status", StringSchema, True),
            #("node", StringSchema, True),
            #("store", StringSchema, True),
            #("peers", IntegerSchema, True),
          ]),
        ),
      ],
      protocol: Http,
    ),
    route(
      id: Nodes,
      method: Get,
      path: "/nodes",
      segments: [Literal("nodes")],
      operation_id: "listPresenceNodes",
      summary: "List connected BEAM nodes",
      description: "Returns the local node and connected BEAM peers. Restricted to the cluster network boundary.",
      tag: "operations",
      route_type: "service",
      visibility: Internal,
      auth: ClusterNetworkPolicy,
      query_parameters: [],
      request_body: NoRequestBody,
      responses: [text_response("200", "Connected BEAM node names.")],
      protocol: Http,
    ),
    route(
      id: WebSocket,
      method: Get,
      path: "/ws",
      segments: [Literal("ws")],
      operation_id: "connectPresenceWebSocket",
      summary: "Open a presence WebSocket",
      description: "Upgrades to a user-scoped WebSocket or, when conv is supplied and membership is valid, a conversation-scoped WebSocket.",
      tag: "presence",
      route_type: "user-generated",
      visibility: Internal,
      auth: ClusterNetworkPolicy,
      query_parameters: [
        ParameterSpec(
          name: "user",
          required: False,
          description: "User identifier. Defaults to anonymous when omitted.",
        ),
        ParameterSpec(
          name: "conv",
          required: False,
          description: "Conversation identifier for a conversation-scoped connection.",
        ),
        ParameterSpec(
          name: "device",
          required: False,
          description: "Optional device identifier used for device-targeted logout.",
        ),
      ],
      request_body: NoRequestBody,
      responses: [
        ResponseSpec(
          status: "101",
          description: "WebSocket protocol upgrade.",
          content_type: "",
          schema: EmptySchema,
        ),
        text_response(
          "403",
          "The user is not a member of the requested conversation.",
        ),
      ],
      protocol: WebSocketProtocol,
    ),
    route(
      id: AddConversationMember,
      method: Post,
      path: "/conv/{conv_id}/members/{user_id}",
      segments: [
        Literal("conv"),
        Variable("conv_id"),
        Literal("members"),
        Variable("user_id"),
      ],
      operation_id: "addPresenceConversationMember",
      summary: "Add a conversation member",
      description: "Durably adds a user to a conversation, refreshes the local cache, and broadcasts the membership change across the cluster.",
      tag: "presence",
      route_type: "user-generated",
      visibility: Internal,
      auth: ClusterNetworkPolicy,
      query_parameters: [],
      request_body: NoRequestBody,
      responses: [text_response("200", "Membership was added.")],
      protocol: Http,
    ),
    route(
      id: RemoveConversationMember,
      method: Delete,
      path: "/conv/{conv_id}/members/{user_id}",
      segments: [
        Literal("conv"),
        Variable("conv_id"),
        Literal("members"),
        Variable("user_id"),
      ],
      operation_id: "removePresenceConversationMember",
      summary: "Remove a conversation member",
      description: "Durably removes a user, refreshes the cache, broadcasts the change, and closes matching conversation WebSockets.",
      tag: "presence",
      route_type: "user-generated",
      visibility: Internal,
      auth: ClusterNetworkPolicy,
      query_parameters: [],
      request_body: NoRequestBody,
      responses: [text_response("200", "Membership was removed.")],
      protocol: Http,
    ),
    route(
      id: ListConversationMembers,
      method: Get,
      path: "/conv/{conv_id}/members",
      segments: [Literal("conv"), Variable("conv_id"), Literal("members")],
      operation_id: "listPresenceConversationMembers",
      summary: "List conversation members",
      description: "Returns the Postgres-backed member list as newline-delimited text.",
      tag: "presence",
      route_type: "user-generated",
      visibility: Internal,
      auth: ClusterNetworkPolicy,
      query_parameters: [],
      request_body: NoRequestBody,
      responses: [text_response("200", "Newline-delimited user identifiers.")],
      protocol: Http,
    ),
    route(
      id: BroadcastConversation,
      method: Post,
      path: "/conv/{conv_id}/broadcast",
      segments: [Literal("conv"), Variable("conv_id"), Literal("broadcast")],
      operation_id: "broadcastPresenceConversation",
      summary: "Broadcast to a conversation",
      description: "Broadcasts the text body to every conversation-scoped WebSocket for current members, across all nodes and the configured NATS bridge.",
      tag: "presence",
      route_type: "user-generated",
      visibility: Internal,
      auth: ClusterNetworkPolicy,
      query_parameters: [],
      request_body: RequestBody(
        required: True,
        description: "Opaque UTF-8 payload forwarded to conversation WebSockets.",
        content_type: "text/plain",
        schema: StringSchema,
      ),
      responses: [
        text_response("200", "Broadcast was queued."),
        text_response("400", "The request body could not be read."),
      ],
      protocol: Http,
    ),
    route(
      id: BroadcastUser,
      method: Post,
      path: "/user/{user_id}/broadcast",
      segments: [Literal("user"), Variable("user_id"), Literal("broadcast")],
      operation_id: "broadcastPresenceUser",
      summary: "Broadcast to a user",
      description: "Broadcasts the text body to every user-scoped WebSocket for the user on every node.",
      tag: "presence",
      route_type: "user-generated",
      visibility: Internal,
      auth: ClusterNetworkPolicy,
      query_parameters: [],
      request_body: RequestBody(
        required: True,
        description: "Opaque UTF-8 payload forwarded to user-scoped WebSockets.",
        content_type: "text/plain",
        schema: StringSchema,
      ),
      responses: [
        text_response("200", "Broadcast was queued."),
        text_response("400", "The request body could not be read."),
      ],
      protocol: Http,
    ),
    route(
      id: LogoutDevice,
      method: Post,
      path: "/user/{user_id}/devices/{device_id}/logout",
      segments: [
        Literal("user"),
        Variable("user_id"),
        Literal("devices"),
        Variable("device_id"),
        Literal("logout"),
      ],
      operation_id: "logoutPresenceDevice",
      summary: "Log out one user device",
      description: "Closes all user- and conversation-scoped WebSockets belonging to one device for one user across the cluster.",
      tag: "presence",
      route_type: "user-generated",
      visibility: Internal,
      auth: ClusterNetworkPolicy,
      query_parameters: [],
      request_body: RequestBody(
        required: False,
        description: "Optional kick reason. Defaults to logout.",
        content_type: "text/plain",
        schema: StringSchema,
      ),
      responses: [text_response("200", "Device logout was queued.")],
      protocol: Http,
    ),
    route(
      id: RuntimeConfigSnapshot,
      method: Get,
      path: "/internal/runtime-config",
      segments: [Literal("internal"), Literal("runtime-config")],
      operation_id: "getPresenceRuntimeConfig",
      summary: "Get applied runtime configuration",
      description: "Returns the runtime-config snapshot currently applied to this process.",
      tag: "runtime-config",
      route_type: "runtime-config",
      visibility: Internal,
      auth: RuntimeConfigServerSecret,
      query_parameters: [],
      request_body: NoRequestBody,
      responses: [
        json_response("200", "Applied runtime-config snapshot.", FreeFormSchema),
        text_response("401", "Server authentication failed."),
      ],
      protocol: Http,
    ),
    route(
      id: RuntimeConfigApply,
      method: Post,
      path: "/internal/update-runtime-config",
      segments: [Literal("internal"), Literal("update-runtime-config")],
      operation_id: "applyPresenceRuntimeConfig",
      summary: "Apply runtime configuration",
      description: "Atomically applies a runtime-config snapshot pushed by the control plane.",
      tag: "runtime-config",
      route_type: "runtime-config",
      visibility: Internal,
      auth: RuntimeConfigServerSecret,
      query_parameters: [],
      request_body: RequestBody(
        required: True,
        description: "Runtime-config apply request.",
        content_type: "application/json",
        schema: FreeFormSchema,
      ),
      responses: [
        json_response(
          "200",
          "Runtime configuration was applied.",
          FreeFormSchema,
        ),
        text_response("401", "Server authentication failed."),
        text_response("400", "The apply request was invalid."),
      ],
      protocol: Http,
    ),
    route(
      id: RuntimeConfigReset,
      method: Post,
      path: "/internal/runtime-config/reset",
      segments: [
        Literal("internal"),
        Literal("runtime-config"),
        Literal("reset"),
      ],
      operation_id: "resetPresenceRuntimeConfig",
      summary: "Reset runtime configuration",
      description: "Drops runtime-config overrides and restores boot-time environment defaults.",
      tag: "runtime-config",
      route_type: "runtime-config",
      visibility: Internal,
      auth: RuntimeConfigServerSecret,
      query_parameters: [],
      request_body: NoRequestBody,
      responses: [
        json_response("200", "Runtime configuration was reset.", FreeFormSchema),
        text_response("401", "Server authentication failed."),
      ],
      protocol: Http,
    ),
  ]
}

fn route(
  id id: RouteId,
  method method: Method,
  path path: String,
  segments segments: List(Segment),
  operation_id operation_id: String,
  summary summary: String,
  description description: String,
  tag tag: String,
  route_type route_type: String,
  visibility visibility: Visibility,
  auth auth: AuthBoundary,
  query_parameters query_parameters: List(ParameterSpec),
  request_body request_body: RequestBodySpec,
  responses responses: List(ResponseSpec),
  protocol protocol: Protocol,
) -> RouteSpec {
  RouteSpec(
    id: id,
    method: method,
    path: path,
    segments: segments,
    operation_id: operation_id,
    summary: summary,
    description: description,
    tag: tag,
    route_type: route_type,
    visibility: visibility,
    auth: auth,
    query_parameters: query_parameters,
    request_body: request_body,
    responses: responses,
    protocol: protocol,
  )
}

fn text_response(status: String, description: String) -> ResponseSpec {
  ResponseSpec(
    status: status,
    description: description,
    content_type: "text/plain",
    schema: StringSchema,
  )
}

fn html_response(status: String, description: String) -> ResponseSpec {
  ResponseSpec(
    status: status,
    description: description,
    content_type: "text/html",
    schema: StringSchema,
  )
}

fn json_response(
  status: String,
  description: String,
  schema: Schema,
) -> ResponseSpec {
  ResponseSpec(
    status: status,
    description: description,
    content_type: "application/json",
    schema: schema,
  )
}

pub fn match_route(
  method: Method,
  path_segments: List(String),
) -> Result(RouteMatch, Nil) {
  find_route(routes(), method, path_segments)
}

fn find_route(
  remaining: List(RouteSpec),
  method: Method,
  path_segments: List(String),
) -> Result(RouteMatch, Nil) {
  case remaining {
    [] -> Error(Nil)
    [candidate, ..rest] ->
      case
        candidate.method == method,
        match_segments(candidate.segments, path_segments, [])
      {
        True, Ok(parameters) ->
          Ok(RouteMatch(route: candidate, parameters: parameters))
        _, _ -> find_route(rest, method, path_segments)
      }
  }
}

fn match_segments(
  expected: List(Segment),
  actual: List(String),
  parameters: List(#(String, String)),
) -> Result(List(#(String, String)), Nil) {
  case expected, actual {
    [], [] -> Ok(list.reverse(parameters))
    [Literal(expected_segment), ..expected_rest],
      [actual_segment, ..actual_rest]
    ->
      case expected_segment == actual_segment {
        True -> match_segments(expected_rest, actual_rest, parameters)
        False -> Error(Nil)
      }
    [Variable(name), ..expected_rest], [actual_segment, ..actual_rest] ->
      match_segments(expected_rest, actual_rest, [
        #(name, actual_segment),
        ..parameters
      ])
    _, _ -> Error(Nil)
  }
}

pub fn parameter(
  parameters: List(#(String, String)),
  name: String,
) -> Result(String, Nil) {
  case parameters {
    [] -> Error(Nil)
    [#(candidate, value), ..rest] ->
      case candidate == name {
        True -> Ok(value)
        False -> parameter(rest, name)
      }
  }
}

pub fn public_openapi_json() -> String {
  openapi_json(Public)
}

pub fn internal_openapi_json() -> String {
  openapi_json(Internal)
}

fn openapi_json(scope: Visibility) -> String {
  let selected = case scope {
    Public -> list.filter(routes(), fn(route) { route.visibility == Public })
    Internal -> routes()
  }

  json.object([
    #("openapi", json.string("3.1.0")),
    #(
      "jsonSchemaDialect",
      json.string("https://json-schema.org/draft/2020-12/schema"),
    ),
    #(
      "info",
      json.object([
        #("title", json.string("gleamlang-presence-server API")),
        #("version", json.string("0.1.0")),
        #(
          "description",
          json.string(case scope {
            Public ->
              "Fail-closed public API contract generated from the same typed route registry used by the Mist dispatcher."
            Internal ->
              "Complete internal API contract generated from the same typed route registry used by the Mist dispatcher."
          }),
        ),
      ]),
    ),
    #(
      "tags",
      json.array(
        ["documentation", "service", "operations", "presence", "runtime-config"],
        fn(tag) { json.object([#("name", json.string(tag))]) },
      ),
    ),
    #("paths", paths_json(selected)),
    #(
      "components",
      json.object([
        #(
          "securitySchemes",
          json.object([
            #(
              "runtimeConfigServerAuth",
              json.object([
                #("type", json.string("apiKey")),
                #("in", json.string("header")),
                #("name", json.string("X-Server-Auth")),
                #(
                  "description",
                  json.string(
                    "Shared server secret required by the runtime-config control-plane receiver.",
                  ),
                ),
              ]),
            ),
          ]),
        ),
      ]),
    ),
    #(
      "x-dd-contract-scope",
      json.string(case scope {
        Public -> "public"
        Internal -> "internal"
      }),
    ),
    #(
      "x-dd-generated-by",
      json.string("gleamlang_presence_server/route_contract"),
    ),
    #("x-dd-native-source-of-truth", json.bool(True)),
    #("x-dd-operation-count", json.int(list.length(selected))),
    #("x-dd-route-count", json.int(list.length(selected))),
  ])
  |> json.to_string
}

fn paths_json(selected: List(RouteSpec)) {
  let paths = unique_paths(selected)
  json.object(
    list.map(paths, fn(path) {
      let operations =
        selected
        |> list.filter(fn(route) { route.path == path })
        |> list.map(fn(route) {
          #(method_name(route.method), operation_json(route))
        })
      #(path, json.object(operations))
    }),
  )
}

fn unique_paths(selected: List(RouteSpec)) -> List(String) {
  list.fold(selected, [], fn(paths, route) {
    case list.contains(paths, route.path) {
      True -> paths
      False -> list.append(paths, [route.path])
    }
  })
}

fn method_name(method: Method) -> String {
  case method {
    Get -> "get"
    Post -> "post"
    Delete -> "delete"
    _ -> "unknown"
  }
}

fn operation_json(route: RouteSpec) {
  let path_parameters = path_parameters(route.segments)
  let parameters = list.append(path_parameters, route.query_parameters)

  let base = [
    #("operationId", json.string(route.operation_id)),
    #("summary", json.string(route.summary)),
    #("description", json.string(route.description)),
    #("tags", json.array([route.tag], json.string)),
    #("parameters", json.array(parameters, parameter_json)),
    #("responses", responses_json(route.responses)),
    #("x-dd-auth", json.string(auth_name(route.auth))),
    #("x-dd-route-type", json.string(route.route_type)),
    #(
      "x-dd-visibility",
      json.string(case route.visibility {
        Public -> "public"
        Internal -> "internal"
      }),
    ),
  ]

  let with_body = list.append(base, request_body_entries(route.request_body))
  let with_security = list.append(with_body, security_entries(route.auth))
  let complete = case route.protocol {
    Http -> with_security
    WebSocketProtocol ->
      list.append(with_security, [
        #("x-dd-protocol", json.string("websocket")),
      ])
  }

  json.object(complete)
}

fn path_parameters(segments: List(Segment)) -> List(ParameterSpec) {
  case segments {
    [] -> []
    [Literal(_), ..rest] -> path_parameters(rest)
    [Variable(name), ..rest] -> [
      ParameterSpec(name: name, required: True, description: "Path identifier."),
      ..path_parameters(rest)
    ]
  }
}

fn parameter_json(parameter: ParameterSpec) {
  json.object([
    #("name", json.string(parameter.name)),
    #(
      "in",
      json.string(case parameter.required {
        True -> "path"
        False -> "query"
      }),
    ),
    #("required", json.bool(parameter.required)),
    #("description", json.string(parameter.description)),
    #("schema", json.object([#("type", json.string("string"))])),
  ])
}

fn request_body_entries(body: RequestBodySpec) {
  case body {
    NoRequestBody -> []
    RequestBody(required, description, content_type, schema) -> [
      #(
        "requestBody",
        json.object([
          #("required", json.bool(required)),
          #("description", json.string(description)),
          #(
            "content",
            json.object([
              #(content_type, json.object([#("schema", schema_json(schema))])),
            ]),
          ),
        ]),
      ),
    ]
  }
}

fn responses_json(responses: List(ResponseSpec)) {
  json.object(
    list.map(responses, fn(response) {
      let base = [#("description", json.string(response.description))]
      let entries = case response.schema {
        EmptySchema -> base
        schema ->
          list.append(base, [
            #(
              "content",
              json.object([
                #(
                  response.content_type,
                  json.object([#("schema", schema_json(schema))]),
                ),
              ]),
            ),
          ])
      }
      #(response.status, json.object(entries))
    }),
  )
}

fn schema_json(schema: Schema) {
  case schema {
    EmptySchema -> json.object([])
    StringSchema -> json.object([#("type", json.string("string"))])
    IntegerSchema -> json.object([#("type", json.string("integer"))])
    BooleanSchema -> json.object([#("type", json.string("boolean"))])
    FreeFormSchema -> json.object([])
    ArraySchema(items) ->
      json.object([
        #("type", json.string("array")),
        #("items", schema_json(items)),
      ])
    ObjectSchema(fields) -> {
      let required =
        fields
        |> list.filter(fn(field) { field.2 })
        |> list.map(fn(field) { field.0 })

      json.object([
        #("type", json.string("object")),
        #(
          "properties",
          json.object(
            list.map(fields, fn(field) { #(field.0, schema_json(field.1)) }),
          ),
        ),
        #("required", json.array(required, json.string)),
        #("additionalProperties", json.bool(False)),
      ])
    }
  }
}

fn security_entries(auth: AuthBoundary) {
  case auth {
    Anonymous -> [#("security", json.array([], json.string))]
    ClusterNetworkPolicy -> []
    RuntimeConfigServerSecret -> [
      #(
        "security",
        json.array(
          [
            json.object([
              #("runtimeConfigServerAuth", json.array([], json.string)),
            ]),
          ],
          fn(value) { value },
        ),
      ),
    ]
  }
}

fn auth_name(auth: AuthBoundary) -> String {
  case auth {
    Anonymous -> "public"
    ClusterNetworkPolicy -> "cluster-network-policy"
    RuntimeConfigServerSecret -> "X-Server-Auth (RUNTIME_CONFIG_SERVER_SECRET)"
  }
}
