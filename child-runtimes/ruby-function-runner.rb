#!/usr/bin/env ruby
# frozen_string_literal: true

require 'json'
require 'net/http'
require 'uri'

MAX_FUNCTION_BODY_BYTES = Integer(ENV.fetch('LAMBDA_FUNCTION_BODY_MAX_BYTES', '262144'))
MAX_INPUT_LINE_BYTES = Integer(ENV.fetch('LAMBDA_CHILD_INPUT_MAX_BYTES', '6291456'))
MAX_RESULT_BYTES = Integer(ENV.fetch('LAMBDA_RESULT_MAX_BYTES', '1048576'))

class LambdaConsole
  %i[debug error info log warn].each do |level|
    define_method(level) do |*args|
      rendered = args.map { |arg| arg.is_a?(String) ? arg : JSON.generate(arg) }.join(' ')
      STDERR.write("[lambda:#{level}] #{rendered}\n")
      STDERR.flush
    end
  end
end

class LambdaContext < BasicObject
  StandardError = ::StandardError

  def initialize(request, lambda_context, console)
    @request = request
    @lambda_context = lambda_context
    @console = console
  end

  def request
    @request
  end

  def context
    @lambda_context
  end

  def console
    @console
  end

  def fetch(url, method: 'GET', headers: {}, body: nil, timeout: 10)
    uri = ::URI.parse(url)
    http = ::Net::HTTP.new(uri.host, uri.port)
    http.use_ssl = uri.scheme == 'https'
    http.open_timeout = timeout
    http.read_timeout = timeout
    request_class = case method.to_s.upcase
                    when 'POST' then ::Net::HTTP::Post
                    when 'PUT' then ::Net::HTTP::Put
                    when 'PATCH' then ::Net::HTTP::Patch
                    when 'DELETE' then ::Net::HTTP::Delete
                    else ::Net::HTTP::Get
                    end
    req = request_class.new(uri)
    headers.each { |key, value| req[key] = value }
    if body
      if body.is_a?(::Hash) || body.is_a?(::Array)
        req['content-type'] ||= 'application/json'
        req.body = ::JSON.generate(body)
      else
        req.body = body.to_s
      end
    end
    response = http.request(req)
    text = response.body.to_s.byteslice(0, MAX_RESULT_BYTES)
    parsed = begin
      ::JSON.parse(text)
    rescue StandardError
      text
    end
    {
      status: response.code.to_i,
      headers: response.each_header.to_h,
      body: parsed
    }
  end
end

def resolve_definition(envelope)
  definition = envelope['definition'] || envelope
  unless definition.is_a?(Hash) && definition['functionBody']
    raise 'lambda definition with functionBody is required'
  end
  status = definition['status']
  raise "lambda function is #{status}" if %w[paused archived].include?(status)

  definition
end

def invoke(line, console)
  envelope = JSON.parse(line)
  definition = resolve_definition(envelope)
  function_body = definition['functionBody'].to_s
  raise 'functionBody is required' if function_body.strip.empty?
  raise 'functionBody exceeds configured byte limit' if function_body.bytesize > MAX_FUNCTION_BODY_BYTES

  request = envelope['request'] || {}
  lambda_context = {
    id: definition['id'],
    invocationId: envelope['invocationId'],
    slug: definition['slug'] || envelope['slug'],
    meta: {
      runtime: definition['runtime'],
      labels: definition['labels'],
      metaData: definition['metaData']
    }.merge(envelope['meta'] || {})
  }
  context = LambdaContext.new(request, lambda_context, console)
  result = context.instance_eval(function_body, '<lambda>', 1)
  {
    ok: true,
    result: result,
    invocationId: lambda_context[:invocationId]
  }
end

def write_result(result)
  encoded = JSON.generate(result)
  if encoded.bytesize > MAX_RESULT_BYTES
    encoded = JSON.generate(ok: false, error: 'lambda result exceeds configured byte limit')
  end
  STDOUT.write("#{encoded}\n")
  STDOUT.flush
end

console = LambdaConsole.new

STDIN.each_line do |line|
  if line.bytesize > MAX_INPUT_LINE_BYTES
    write_result(ok: false, error: 'lambda input exceeds configured byte limit')
    next
  end

  line = line.strip
  next if line.empty?

  begin
    write_result(invoke(line, console))
  rescue StandardError => e
    STDERR.write("#{e.class}: #{e.message}\n")
    STDERR.write(e.backtrace.join("\n")) if e.backtrace
    STDERR.write("\n")
    STDERR.flush
    write_result(ok: false, error: e.message)
  end
end
