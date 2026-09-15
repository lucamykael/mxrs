# frozen_string_literal: true
require_relative 'command_oracle_support'

o = CommandOracle.new('project')
# Both workspace representations live in the same disposable directory. The
# native project.rb/modules and Cargo.toml/src tree encode the same inventory;
# no Ruby or Rust project source is executed by either inspection command.
def write_project_pair(root, version: nil)
  FileUtils.mkdir_p(File.join(root, 'src/domain/modules'))
  File.write(File.join(root, 'Cargo.toml'), '[package]')
  File.write(File.join(root, 'project.rb'), version ? "mendix_version '#{version}'\nraise 'must never execute'\n" : "raise 'must never execute'\n")
  File.write(File.join(root, 'src/lib.rs'), version ? "#[mxrs::application(version = \"#{version}\")]\npub mod domain;\n" : "pub mod domain;\n")
  File.write(File.join(root, 'src/domain/mod.rs'), '')
end

def inventory(o, root, default: false)
  args = default ? ['inspect'] : ['inspect', root]
  options = default ? { chdir: root } : {}
  native = JSON.parse(o.success(true, *args, '--json', **options))
  rust = JSON.parse(o.success(false, *args, '--json', **options))
  expected = native.reject { |key, _| key == 'project_file' }.merge('manifest' => native.fetch('project_file'))
  o.check('complete workspace facts after documented language mapping') { rust.reject { |key, _| %w[domain_module layout].include?(key) } == expected }
  o.check('Cargo-specific source and layout facts') do
    rust['domain_module'] == File.file?(File.join(File.expand_path(root), 'src/domain/mod.rs')) && %w[incomplete pre-layered layered].include?(rust['layout'])
  end
  [true, false].each do |implementation|
    json = implementation ? native : rust
    # Both text contracts render every JSON fact in the documented field order.
    output = o.success(implementation, *args, **options)
    parsed = output.lines.to_h { _1.chomp.split(': ', 2) }
    o.check('human report includes every JSON fact') do
      parsed == json.transform_values { |value| Array(value).join(', ') }
    end
  end
  o.check('--no-progress') { o.success(false, *args, '--no-progress', **options) == o.success(false, *args, **options) }
end

Dir.mktmpdir('mxrs-project-oracle-') do |root|
  workspace = File.join(root, 'Workspace with spaces')
  FileUtils.mkdir_p(workspace)
  o.readonly(root) { inventory(o, workspace) }
  o.readonly(root) { inventory(o, File.join(root, 'absent')) }
  write_project_pair(workspace)
  o.readonly(root) { inventory(o, workspace) }
  write_project_pair(workspace, version: '11.12.1')
  %w[zulu alpha .hidden].each do |name|
    FileUtils.mkdir_p(File.join(workspace, 'modules', name))
    File.write(File.join(workspace, 'modules', name, 'module.rb'), "raise 'must never execute'")
    FileUtils.mkdir_p(File.join(workspace, 'src/domain/modules', name))
    File.write(File.join(workspace, 'src/domain/modules', name, 'mod.rs'), '')
  end
  # Bare folders are not module definitions; MPR inventory is lexical and does
  # not open/validate project databases.
  FileUtils.mkdir_p(File.join(workspace, 'src/domain/modules', 'ignored'))
  %w[Zulu.mpr Alpha.mpr .hidden.mpr].each { File.write(File.join(workspace, _1), 'inventory only') }
  %w[.mxrb .mxrs].each do |name|
    FileUtils.mkdir_p(File.join(workspace, name))
    File.write(File.join(workspace, name, 'scaffolds.json'), JSON.generate('scaffolds' => {
      'module:Zulu' => { 'files' => [] }, 'entity:Alpha.Record' => { 'files' => [] }
    }))
  end
  o.readonly(root) do
    inventory(o, workspace)
    inventory(o, workspace, default: true)
    inventory(o, File.join(workspace, 'ignored/..'))
  end
  File.write(File.join(workspace, 'src/lib.rs'), <<~RUST)
    // #[mxrs::application(version = "wrong")]
    pub mod domain;
    #[mxrs::application(
      project = crate::build,
      version = r"11.12.1",
    )]
    pub struct Application;
  RUST
  o.readonly(root) { inventory(o, workspace) }
  File.write(File.join(workspace, 'mxrs.toml'), "mendix_version = \"11.13.0\"\n")
  File.write(File.join(workspace, 'project.rb'), "mendix_version '11.13.0'\n")
  o.readonly(root) { inventory(o, workspace) }
  # Cargo extension: build/ output joins root MPRs, always sorted.
  FileUtils.mkdir_p(File.join(workspace, 'build'))
  File.write(File.join(workspace, 'build/Generated.mpr'), 'inventory only')
  o.readonly(root) do
    rust = JSON.parse(o.success(false, 'inspect', workspace, '--json'))
    o.check('build output inventory') { rust['mprs'] == Dir[File.join(workspace, '{,build/}*.mpr')].sort }
  end
  %w[.mxrb .mxrs].each { File.write(File.join(workspace, _1, 'scaffolds.json'), '{') }
  o.readonly(root) { o.failure('inspect', workspace) }
  [[], ['unknown']].each { o.failure(*_1) }
  [['inspect', root, 'extra'], ['inspect', root, '--unknown'], ['inspect', root, '--json', '--json']].each { o.failure(*_1, native: false) }
  o.help
end
o.finish
