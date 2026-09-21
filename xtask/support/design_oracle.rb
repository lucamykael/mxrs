# frozen_string_literal: true

# Differential oracle for `design`: `scan` and `migrate` must report the same
# token inventory, quality signals, and replacement plans over the same theme
# assets (human output compared modulo the [mxrb]/[mxrs] prefix, JSON parsed
# and compared structurally), `migrate --apply` must leave byte-identical
# stylesheets behind, and `init` must materialize the same theme kit in a
# native project and a Cargo project (custom-variables.scss differs only in
# the product name inside its comment).
require_relative 'command_oracle_support'

o = CommandOracle.new('design')
workspace = File.expand_path('../..', __dir__)

def command!(*args, **options)
  out, err, status = Open3.capture3(*args, **options)
  raise "command failed: #{args.inspect}: #{err}" unless status.success?
  out
end

def theme_fixture(root)
  FileUtils.mkdir_p(File.join(root, 'theme/web'))
  FileUtils.mkdir_p(File.join(root, 'themesource/atlas/web'))
  FileUtils.mkdir_p(File.join(root, 'widgets/inner'))
  File.write(File.join(root, 'theme/web/custom-variables.scss'), <<~SCSS)
    // $commented: #111111;
    $brand-primary: #264ae5;
    $brand-secondary: #F6A21E; $spacing-medium: 1rem;
    --alias: var(--missing-reference) solid;
    $reference: var(--alias-two);
  SCSS
  File.write(File.join(root, 'theme/web/_theme-dark.scss'), <<~SCSS)
    --surface: #101820;
    --alias-two: #ffffff;
  SCSS
  File.write(File.join(root, 'widgets/inner/widget.css'), <<~CSS)
    --widget-accent: #abc;
  CSS
  File.write(File.join(root, 'themesource/atlas/web/design-properties.json'), '{"pages": []}')
  FileUtils.mkdir_p(File.join(root, 'themesource/atlas/web/broken'))
  File.write(File.join(root, 'themesource/atlas/web/broken/design-properties.json'), 'not json')
end

def model_fixture(path)
  FileUtils.mkdir_p(File.dirname(path))
  Mxrb.define(path) do
    mendix_version '11.12.1'
    self.module(:Zulu) { entity(:Order) { string :Number } }
  end
end

normalize_prefix = lambda do |lhs, rhs|
  [lhs.gsub(/^\[mxrb\]/, '[CLI]'), rhs.gsub(/^\[mxrs\]/, '[CLI]')]
end
parse_json = lambda { |lhs, rhs| [JSON.parse(lhs), JSON.parse(rhs)] }

Dir.mktmpdir('mxrs-design-oracle-') do |root|
  project = File.join(root, 'scan')
  mpr = File.join(project, 'Project.mpr')
  model_fixture(mpr)
  theme_fixture(project)

  o.readonly(project) do
    lhs, rhs = normalize_prefix.call(o.success(true, 'scan', mpr), o.success(false, 'scan', mpr))
    o.check('scan human output') { lhs == rhs }
    left, right = parse_json.call(o.success(true, 'scan', mpr, '--json'), o.success(false, 'scan', mpr, '--json'))
    o.check("scan JSON facts: #{left.inspect} != #{right.inspect}") { left == right }
    o.check('scan finds the fixture inventory') do
      left['tokens'].size == 8 && left['themes'] == ['dark'] &&
        left['literal_colors'].include?('$brand-primary') &&
        left['unresolved_references'].map { _1['reference'] }.include?('--missing-reference') &&
        left['catalogs'].values.include?(nil) # unparsable catalog stays null
    end
    lhs, rhs = normalize_prefix.call(
      o.success(true, 'migrate', mpr, '#264ae5', '$brand-primary'),
      o.success(false, 'migrate', mpr, '#264ae5', '$brand-primary')
    )
    o.check('migrate preview output') { lhs == rhs }
    left, right = parse_json.call(
      o.success(true, 'migrate', mpr, '#264ae5', '$brand-primary', '--json'),
      o.success(false, 'migrate', mpr, '#264ae5', '$brand-primary', '--json')
    )
    o.check('migrate preview JSON') { left == right && left['applied'] == false }
  end

  # --apply on two identical copies must leave identical trees behind.
  native_copy = File.join(root, 'native-apply')
  rust_copy = File.join(root, 'rust-apply')
  [native_copy, rust_copy].each do |copy|
    FileUtils.mkdir_p(copy)
    FileUtils.cp_r(Dir.glob(File.join(project, '*')), copy)
  end
  lhs, rhs = normalize_prefix.call(
    o.success(true, 'migrate', File.join(native_copy, 'Project.mpr'), '#264ae5', '$brand-primary', '--apply'),
    o.success(false, 'migrate', File.join(rust_copy, 'Project.mpr'), '#264ae5', '$brand-primary', '--apply')
  )
  o.check('migrate apply output') { lhs == rhs }
  o.check('migrated stylesheets are byte-identical') do
    o.files(native_copy).except('Project.mpr') == o.files(rust_copy).except('Project.mpr')
  end
  o.check('the literal is gone after apply') do
    !File.read(File.join(rust_copy, 'theme/web/custom-variables.scss')).include?('#264ae5')
  end

  # Shared refusals.
  o.failure('scan', File.join(root, 'missing.mpr'))
  garbage = File.join(root, 'garbage.mpr')
  File.binwrite(garbage, 'not a model store')
  o.failure('scan', garbage)
  o.failure('migrate', mpr)                       # literal and token missing
  o.failure('migrate', mpr, '#264ae5')            # token missing
  o.failure('bogus', mpr)                         # unknown action
  o.failure                                       # no action at all

  # init pairing: same theme kit in a native project and a Cargo project.
  ruby = File.join(root, 'ruby-init')
  FileUtils.mkdir_p(ruby)
  File.write(File.join(ruby, 'project.rb'), "require 'mxrb'\n\nMxrb.define('Out.mpr') do\n  mendix_version '11.12.1'\nend\n")
  rust = File.join(root, 'rust-init')
  command!(o.mxrs, 'new', "Design #{File.basename(root)}", '--output', rust, '--mxrs-workspace', workspace)
  command!(RbConfig.ruby, o.mxrb, 'design', 'init', '--target', ruby)
  command!(o.mxrs, 'design', 'init', '--target', rust)
  %w[
    theme/web/main.scss
    theme/web/exclusion-variables.scss
    theme/web/settings.json
    theme-cache/web/theme.compiled.css
  ].each do |relative|
    o.check("init pairs #{relative}") do
      File.read(File.join(ruby, relative)) == File.read(File.join(rust, relative))
    end
  end
  o.check('init pairs custom-variables modulo the product name') do
    File.read(File.join(ruby, 'theme/web/custom-variables.scss')).gsub('mxrb', 'mxrs') ==
      File.read(File.join(rust, 'theme/web/custom-variables.scss'))
  end
  # Re-running init never clobbers an edited theme.
  File.write(File.join(rust, 'theme/web/custom-variables.scss'), "$edited: 1;\n")
  command!(o.mxrs, 'design', 'init', '--target', rust)
  o.check('init preserves edited assets') do
    File.read(File.join(rust, 'theme/web/custom-variables.scss')) == "$edited: 1;\n"
  end

  o.help
end
o.finish
