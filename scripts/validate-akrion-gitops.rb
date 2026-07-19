# frozen_string_literal: true

require 'json'
require 'open3'
require 'yaml'

ROOT = File.expand_path('..', __dir__)
Dir.chdir(ROOT)

release = JSON.parse(File.read('remote/argocd/akrion/release.json'))
raise 'unsupported Akrion release schema' unless release['schemaVersion'] == 1

expected_repositories = {
  'backend' => 'akrion-sim/akrion-backend.rs',
  'web' => 'akrion-sim/akrion-web-server.rs',
  'soccer' => 'akrion-sim/akrion-soccer-engine-rs',
  'des' => 'ORESoftware/discrete-event-system.rs'
}.freeze

revisions = expected_repositories.to_h do |component, repository|
  entry = release.fetch('components').fetch(component)
  raise "wrong repository for #{component}" unless entry['repository'] == repository

  revision = entry.fetch('revision')
  raise "invalid #{component} revision" unless revision.match?(/\A[0-9a-f]{40}\z/)
  [component, revision]
end

def gitlink(path)
  output, status = Open3.capture2e('git', 'ls-files', '--stage', '--', path)
  raise "unable to read gitlink #{path}: #{output}" unless status.success?

  fields = output.split
  raise "#{path} is not a gitlink" unless fields.length >= 4 && fields[0] == '160000'
  fields[1]
end

expected_gitlinks = {
  'remote/deployments/soccer-rs' => revisions.fetch('backend'),
  'remote/deployments/akrion-web-server-rs' => revisions.fetch('web'),
  'remote/submodules/soccer-sim-game-engine.rs' => revisions.fetch('soccer'),
  'remote/submodules/discrete-event-system.rs' => revisions.fetch('des')
}
expected_gitlinks.each do |path, revision|
  actual = gitlink(path)
  raise "#{path} points to #{actual}, expected #{revision}" unless actual == revision
end

gitmodules = File.read('.gitmodules')
canonical_soccer_url = 'https://github.com/akrion-sim/akrion-soccer-engine-rs.git'
raise 'soccer submodule is not canonical Akrion source' unless gitmodules.include?(canonical_soccer_url)

def documents(path)
  YAML.load_stream(File.read(path)).compact
end

def only_document(path)
  docs = documents(path)
  raise "expected exactly one YAML document in #{path}" unless docs.length == 1
  docs.first
end

def deployment_container(document, name)
  containers = document.dig('spec', 'template', 'spec', 'containers') || []
  containers.find { |container| container['name'] == name } || raise("missing container #{name}")
end

def cronjob_container(document, name)
  containers = document.dig('spec', 'jobTemplate', 'spec', 'template', 'spec', 'containers') || []
  containers.find { |container| container['name'] == name } || raise("missing container #{name}")
end

def env_value(container, name)
  entry = (container['env'] || []).find { |item| item['name'] == name }
  entry&.fetch('value', nil)
end

backend = only_document('remote/argocd/dd-next-runtime/dd-soccer-rs.deployment.yaml')
backend_annotations = backend.dig('spec', 'template', 'metadata', 'annotations') || {}
{
  'dd.dev/soccer-server-revision' => revisions.fetch('backend'),
  'dd.dev/soccer-engine-revision' => revisions.fetch('soccer'),
  'dd.dev/des-engine-revision' => revisions.fetch('des')
}.each do |key, value|
  raise "backend annotation #{key} is not pinned" unless backend_annotations[key] == value
end
backend_container = deployment_container(backend, 'soccer-rs')
raise 'backend soccer ref is not pinned' unless env_value(backend_container, 'SOCCER_ENGINE_GIT_REF') == revisions.fetch('soccer')
raise 'backend DES ref is not pinned' unless env_value(backend_container, 'DES_ENGINE_GIT_REF') == revisions.fetch('des')

web = only_document('remote/argocd/dd-next-runtime/dd-akrion-web-server-rs.deployment.yaml')
web_revision = web.dig('spec', 'template', 'metadata', 'annotations', 'dd.dev/akrion-web-server-revision')
raise 'web revision annotation is not pinned' unless web_revision == revisions.fetch('web')

{
  'remote/argocd/dd-next-runtime/dd-soccer-learning-queue.cronjob.yaml' => 'soccer-learning-queue',
  'remote/argocd/dd-next-runtime/dd-soccer-tournament-nightly.cronjob.yaml' => 'soccer-tournament'
}.each do |path, container_name|
  workload = only_document(path)
  container = cronjob_container(workload, container_name)
  raise "#{path} soccer ref is not pinned" unless env_value(container, 'SOCCER_SOURCE_REF') == revisions.fetch('soccer')
  raise "#{path} DES ref is not pinned" unless env_value(container, 'SOCCER_ENGINE_SOURCE_REF') == revisions.fetch('des')
end

def render(path)
  output, status = Open3.capture2e(
    'kubectl', 'kustomize', '--load-restrictor=LoadRestrictionsNone', path
  )
  raise "Kustomize render failed for #{path}:\n#{output}" unless status.success?
  YAML.load_stream(output).compact
end

{
  'aws' => 1,
  'hetzner' => 0
}.each do |cluster, replicas|
  path = "remote/argocd/akrion-training/overlays/#{cluster}"
  docs = render(path)
  learner = docs.find do |doc|
    doc['kind'] == 'Deployment' && doc.dig('metadata', 'name') == 'dd-soccer-learning-rds-continuous'
  end
  watcher = docs.find do |doc|
    doc['kind'] == 'Deployment' && doc.dig('metadata', 'name') == 'dd-soccer-commit-watcher'
  end
  raise "#{cluster} learner is missing" unless learner
  raise "#{cluster} learner replicas are unsafe" unless learner.dig('spec', 'replicas') == replicas
  raise "#{cluster} commit-watcher must be disabled" unless watcher&.dig('spec', 'replicas') == 0
  sync_options = learner.dig('metadata', 'annotations', 'argocd.argoproj.io/sync-options').to_s
  raise "#{cluster} learner must use partial Server-Side Apply" unless sync_options.include?('ServerSideApply=true') && sync_options.include?('Validate=false')

  annotations = learner.dig('spec', 'template', 'metadata', 'annotations') || {}
  raise "#{cluster} learner soccer annotation is stale" unless annotations['dd.dev/soccer-engine-revision'] == revisions.fetch('soccer')
  raise "#{cluster} learner DES annotation is stale" unless annotations['dd.dev/des-engine-revision'] == revisions.fetch('des')
  container = deployment_container(learner, 'soccer-learning')
  raise "#{cluster} learner source repository is not canonical" unless env_value(container, 'SOCCER_SOURCE_REPO') == canonical_soccer_url
  raise "#{cluster} learner soccer ref is stale" unless env_value(container, 'SOCCER_SOURCE_REF') == revisions.fetch('soccer')
  raise "#{cluster} learner DES ref is stale" unless env_value(container, 'SOCCER_ENGINE_SOURCE_REF') == revisions.fetch('des')
end

{
  'aws' => 'remote/argocd/akrion-training/overlays/aws',
  'hetzner' => 'remote/argocd/akrion-training/overlays/hetzner'
}.each do |cluster, path|
  app = documents("remote/argocd/clusters/#{cluster}/applications.yaml").find do |doc|
    doc['kind'] == 'Application' && doc.dig('metadata', 'name') == 'dd-akrion-training'
  end
  raise "#{cluster} Argo application is missing" unless app
  raise "#{cluster} Argo application does not track dev" unless app.dig('spec', 'source', 'targetRevision') == 'dev'
  raise "#{cluster} Argo application path is wrong" unless app.dig('spec', 'source', 'path') == path
  raise "#{cluster} Argo application is not self-healing" unless app.dig('spec', 'syncPolicy', 'automated', 'selfHeal') == true
  options = app.dig('spec', 'syncPolicy', 'syncOptions') || []
  raise "#{cluster} Argo application must use Server-Side Apply" unless options.include?('ServerSideApply=true')
  raise "#{cluster} Argo application must disable client validation for partial resources" unless options.include?('Validate=false')
end

puts "Akrion GitOps contract is valid: backend=#{revisions.fetch('backend')} " \
     "web=#{revisions.fetch('web')} soccer=#{revisions.fetch('soccer')} des=#{revisions.fetch('des')}"
