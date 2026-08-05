#!/usr/bin/env ruby
# frozen_string_literal: true

require 'yaml'

ROOT = File.expand_path('..', __dir__)

PRODUCTS = {
  'hypesiege' => {
    repos: {
      'dd-hypesiege-api-server' => 'git@github.com:hypesiege/hypesiege-api-server.rs.git',
      'dd-hypesiege-web-server' => 'git@github.com:hypesiege/hypesiege-web-server.rs.git'
    }
  },
  'streempilot' => {
    repos: {
      'dd-streempilot-api-server' => 'git@github.com:StreemPilot/streempilot-api-server.rs.git',
      'dd-streempilot-web-server' => 'git@github.com:StreemPilot/sp-web-mash.git'
    }
  }
}.freeze

def read_documents(relative_path)
  path = File.join(ROOT, relative_path)
  raise "missing #{relative_path}" unless File.file?(path)

  YAML.load_stream(File.read(path)).compact
end

def assert(condition, message)
  raise message unless condition
end

def assert_automated_sync(document, label)
  automated = document.dig('spec', 'syncPolicy', 'automated')
  assert(automated == { 'prune' => true, 'selfHeal' => true },
         "#{label} must prune and self-heal")
  options = document.dig('spec', 'syncPolicy', 'syncOptions') || []
  assert(options.include?('ServerSideApply=true'),
         "#{label} must use server-side apply")
  assert(!options.include?('CreateNamespace=true'),
         "#{label} must not create platform-owned namespaces")
end

PRODUCTS.each do |product, config|
  tenant_path = "remote/argocd/projects/#{product}.tenant.yaml"
  project_path = "remote/argocd/projects/#{product}.appproject.yaml"
  children_path = "remote/argocd/apps/#{product}.applications.yaml"
  root_path = "remote/argocd/apps/#{product}-root.application.yaml"

  tenant_documents = read_documents(tenant_path)
  kinds = tenant_documents.map { |document| document['kind'] }
  %w[Namespace ResourceQuota LimitRange NetworkPolicy].each do |kind|
    assert(kinds.include?(kind), "#{tenant_path} is missing #{kind}")
  end
  namespace = tenant_documents.find { |document| document['kind'] == 'Namespace' }
  assert(namespace.dig('metadata', 'name') == product,
         "#{product} Namespace has the wrong name")
  deny = tenant_documents.find { |document| document['kind'] == 'NetworkPolicy' }
  assert(deny.dig('metadata', 'namespace') == product,
         "#{product} default-deny policy has the wrong namespace")
  assert(deny.dig('spec', 'podSelector') == {},
         "#{product} default-deny must select every pod")
  assert(deny.dig('spec', 'policyTypes') == ['Ingress'],
         "#{product} default-deny must cover ingress")
  assert(!deny.dig('spec').key?('ingress'),
         "#{product} default-deny must not contain an allow rule")

  project = read_documents(project_path).fetch(0)
  assert(project['kind'] == 'AppProject', "#{project_path} is not an AppProject")
  assert(project.dig('metadata', 'name') == product,
         "#{product} AppProject has the wrong name")
  assert(project.dig('metadata', 'namespace') == 'argocd',
         "#{product} AppProject must live in argocd")
  assert(project.dig('spec', 'sourceRepos').sort == config[:repos].values.sort,
         "#{product} AppProject source repositories drifted")
  assert(project.dig('spec', 'destinations') == [{
           'server' => 'https://kubernetes.default.svc',
           'namespace' => product
         }], "#{product} AppProject destination drifted")
  assert(project.dig('spec', 'clusterResourceWhitelist') == [],
         "#{product} AppProject must deny cluster-scoped resources")

  children = read_documents(children_path)
  assert(children.length == config[:repos].length,
         "#{product} must have one child Application per Rust service")
  children.each do |child|
    name = child.dig('metadata', 'name')
    expected_repo = config[:repos][name]
    assert(expected_repo, "unexpected #{product} child Application #{name.inspect}")
    assert(child['kind'] == 'Application', "#{name} is not an Application")
    assert(child.dig('metadata', 'namespace') == 'argocd',
           "#{name} must live in argocd")
    assert(child.dig('spec', 'project') == product,
           "#{name} must use the #{product} AppProject")
    assert(child.dig('spec', 'source', 'repoURL') == expected_repo,
           "#{name} points at the wrong repository")
    assert(child.dig('spec', 'source', 'path') == 'deploy/k8s',
           "#{name} must render app-owned deploy/k8s manifests")
    assert(child.dig('spec', 'source', 'targetRevision') == 'main',
           "#{name} bootstrap revision must be main")
    assert(child.dig('spec', 'destination', 'namespace') == product,
           "#{name} has the wrong destination namespace")
    assert(!expected_repo.include?('-infra'), "#{name} must not render an infra repo")
    assert(!expected_repo.include?('monorepo'), "#{name} must not render a monorepo")
    assert_automated_sync(child, name)
  end

  root = read_documents(root_path).fetch(0)
  assert(root['kind'] == 'Application', "#{root_path} is not an Application")
  assert(root.dig('metadata', 'name') == "#{product}-root",
         "#{product} root has the wrong name")
  assert(root.dig('spec', 'project') == 'default',
         "#{product} root must bootstrap through the platform project")
  assert(root.dig('spec', 'source', 'repoURL') ==
         'git@github.com:ORESoftware/k8s-cluster.git',
         "#{product} root must be sourced from k8s-cluster")
  assert(root.dig('spec', 'source', 'path') == 'remote/argocd',
         "#{product} root has the wrong source path")
  include_glob = root.dig('spec', 'source', 'directory', 'include')
  %W[
    projects/#{product}.tenant.yaml
    projects/#{product}.appproject.yaml
    apps/#{product}.applications.yaml
  ].each do |managed_file|
    assert(include_glob.include?(managed_file),
           "#{product} root does not manage #{managed_file}")
  end
  assert(!include_glob.include?("#{product}-root.application.yaml"),
         "#{product} root must not recursively manage itself")
  assert_automated_sync(root, "#{product}-root")
end

architecture = File.read(File.join(ROOT, 'docs/hypesiege-streempilot-gitops.md'))
%w[Cloudflare Supabase shared-auth Rust Argo].each do |term|
  assert(architecture.include?(term), "architecture document is missing #{term}")
end
assert(architecture.include?('StreemPilot/sp-web-mash'),
       'architecture document must record the transferred StreemPilot web repository')

puts 'Hypesiege and StreemPilot app-of-apps contract passed'
