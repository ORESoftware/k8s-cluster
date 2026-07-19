# frozen_string_literal: true

require 'json'
require 'time'

ROOT = File.expand_path('..', __dir__)
Dir.chdir(ROOT)

REVISION_ENV = {
  'backend' => 'AKRION_BACKEND_REVISION',
  'web' => 'AKRION_WEB_REVISION',
  'soccer' => 'AKRION_SOCCER_REVISION',
  'des' => 'AKRION_DES_REVISION'
}.freeze

revisions = REVISION_ENV.to_h do |component, variable|
  revision = ENV.fetch(variable, '').strip
  abort "#{variable} must be a full 40-character commit SHA" unless revision.match?(/\A[0-9a-f]{40}\z/)
  [component, revision]
end

def replace_one(path, pattern, description)
  text = File.read(path)
  matches = text.scan(pattern).length
  raise "expected one #{description} in #{path}, found #{matches}" unless matches == 1

  updated = text.sub(pattern) { yield(Regexp.last_match) }
  File.write(path, updated) if updated != text
end

def set_mapping_value(path, key, value, quoted: false)
  pattern = /^(\s*#{Regexp.escape(key)}:\s*).+$/
  replace_one(path, pattern, key) do |match|
    rendered = quoted ? "'#{value}'" : value
    "#{match[1]}#{rendered}"
  end
end

def set_env_value(path, name, value)
  pattern = /^(\s*- name: #{Regexp.escape(name)}\n\s+value:)\s+.*$/
  replace_one(path, pattern, "environment variable #{name}") do |match|
    "#{match[1]} #{value}"
  end
end

backend_manifest = 'remote/argocd/dd-next-runtime/dd-soccer-rs.deployment.yaml'
web_manifest = 'remote/argocd/dd-next-runtime/dd-akrion-web-server-rs.deployment.yaml'
queue_manifest = 'remote/argocd/dd-next-runtime/dd-soccer-learning-queue.cronjob.yaml'
tournament_manifest = 'remote/argocd/dd-next-runtime/dd-soccer-tournament-nightly.cronjob.yaml'
training_base = 'remote/argocd/akrion-training/base/kustomization.yaml'

set_mapping_value(backend_manifest, 'dd.dev/soccer-server-revision', revisions.fetch('backend'), quoted: true)
set_mapping_value(backend_manifest, 'dd.dev/soccer-engine-revision', revisions.fetch('soccer'), quoted: true)
set_mapping_value(backend_manifest, 'dd.dev/des-engine-revision', revisions.fetch('des'), quoted: true)
set_env_value(backend_manifest, 'SOCCER_ENGINE_GIT_REF', revisions.fetch('soccer'))
set_env_value(backend_manifest, 'DES_ENGINE_GIT_REF', revisions.fetch('des'))

set_mapping_value(web_manifest, 'dd.dev/akrion-web-server-revision', revisions.fetch('web'), quoted: true)

[queue_manifest, tournament_manifest].each do |path|
  set_env_value(path, 'SOCCER_SOURCE_REF', revisions.fetch('soccer'))
  set_env_value(path, 'SOCCER_ENGINE_SOURCE_REF', revisions.fetch('des'))
end

set_mapping_value(training_base, 'dd.dev/soccer-engine-revision', revisions.fetch('soccer'))
set_mapping_value(training_base, 'dd.dev/des-engine-revision', revisions.fetch('des'))
set_env_value(training_base, 'SOCCER_SOURCE_REF', revisions.fetch('soccer'))
set_env_value(training_base, 'SOCCER_ENGINE_SOURCE_REF', revisions.fetch('des'))

release_path = 'remote/argocd/akrion/release.json'
release = JSON.parse(File.read(release_path))
release_changed = revisions.any? do |component, revision|
  release.dig('components', component, 'revision') != revision
end

if release_changed
  revisions.each do |component, revision|
    release.fetch('components').fetch(component)['revision'] = revision
  end
  release['promotedAt'] = Time.now.utc.iso8601
  File.write(release_path, "#{JSON.pretty_generate(release)}\n")
end

gitlinks = {
  'remote/deployments/soccer-rs' => revisions.fetch('backend'),
  'remote/deployments/akrion-web-server-rs' => revisions.fetch('web'),
  'remote/submodules/soccer-sim-game-engine.rs' => revisions.fetch('soccer'),
  'remote/submodules/discrete-event-system.rs' => revisions.fetch('des')
}

gitlinks.each do |path, revision|
  ok = system('git', 'update-index', '--add', '--cacheinfo', "160000,#{revision},#{path}")
  abort "unable to update gitlink #{path}" unless ok
end

files = [
  backend_manifest,
  web_manifest,
  queue_manifest,
  tournament_manifest,
  training_base,
  release_path
]
abort 'unable to stage Akrion release manifests' unless system('git', 'add', '--', *files)

puts "promoted backend=#{revisions.fetch('backend')} web=#{revisions.fetch('web')} " \
     "soccer=#{revisions.fetch('soccer')} des=#{revisions.fetch('des')}"
