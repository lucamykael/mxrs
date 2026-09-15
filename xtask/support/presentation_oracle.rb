# frozen_string_literal: true
require_relative 'command_oracle_support'

o = CommandOracle.new('presentation')
workspace = File.expand_path('../..', __dir__)

def command!(*args, **options)
  out, err, status = Open3.capture3(*args, **options)
  raise "command failed: #{args.first}: #{err}" unless status.success?
  out
end

def pair(o, root, workspace)
  ruby = File.join(root, 'ruby')
  rust = File.join(root, 'rust')
  FileUtils.mkdir_p(File.join(ruby, 'modules/Sales'))
  File.write(File.join(ruby, 'project.rb'), <<~SOURCE)
    require 'mxrb'
    Mxrb.define(ENV.fetch('MXRB_OUTPUT_PATH')) do
      mendix_version '11.12.1'
      evaluate File.join(__dir__, 'modules', 'Sales', 'module.rb')
    end
  SOURCE
  File.write(File.join(ruby, 'modules/Sales/module.rb'), "self.module(:Sales) do\nend\n")
  command!(o.mxrs, 'new', "Presentation #{File.basename(root)}", '--output', rust, '--mxrs-workspace', workspace)
  command!(o.mxrs, 'module', 'new', 'Sales', '--target', rust)
  [ruby, rust]
end

def normalized(value)
  case value
  when Hash then value.reject { |key, item| key == '$ID' || item.nil? }.transform_values { normalized(_1) }
  when Array then value.map { normalized(_1) }
  else value
  end
end

def layout(path)
  Mxrb.open(path) do |project|
    mod = project.modules.find { _1.name == 'Sales' }
    unit = project.all_units.find do |unit|
      doc = project.parse_bson(unit)
      unit['ContainerID'] == mod.id && doc['Name'] == 'ApplicationLayout'
    end or raise 'missing Sales.ApplicationLayout'
    codec = Mxrb::Forms::MprCodec.new
    normalized(codec.encode(codec.decode(project.parse_bson(unit))))
  end
end

def check_model(o, ruby, rust, cache, root)
  left = File.join(root, 'native-output/Project.mpr')
  right = File.join(root, 'rust-output/Project.mpr')
  FileUtils.mkdir_p(File.dirname(left)); FileUtils.mkdir_p(File.dirname(right))
  command!({'MXRB_OUTPUT_PATH' => left}, RbConfig.ruby, '-I', File.join(File.dirname(o.mxrb), '../lib'), File.join(ruby, 'project.rb'))
  _, err, status = Open3.capture3({'CARGO_TARGET_DIR' => cache}, 'cargo', 'run', '--offline', '--quiet', '--', right, chdir: rust)
  o.check("generated Rust compiles without warnings: #{err}") { status.success? && err.empty? }
  expected = layout(left); actual = layout(right)
  # MXRS explicitly declares the layout parameter used by generated page
  # calls; native layout authorship derives it from the Main placeholder.
  o.check('explicit Main layout parameter') { actual.delete('Parameters') == [2, { '$Type' => 'Forms$LayoutParameter', 'Name' => 'Main' }] }
  o.check('native parameter is implicit') { !expected.key?('Parameters') }
  if expected != actual
    diff = Mxrb::Compare::Comparator.new(left, right).send(:diff_values, expected, actual)
    warn diff.map(&:to_s).join("\n")
  end
  o.check('complete native layout after shared strict Forms decoding/encoding, excluding generated IDs and null/absent fields') { expected == actual }
end

def preview(o, native, root)
  before = o.files(root)
  payload = JSON.parse(o.success(native, 'init', 'Sales', '--target', root, '--dry-run', '--json'))
  o.check('preview facts') { payload.values_at('kind', 'name', 'dry_run') == ['presentation', 'Sales', true] }
  out = o.success(native, 'init', 'Sales', '--target', root, '--dry-run')
  expected = payload['files'].map { "  would create  #{_1}\n" }.join + payload['updated'].map { "  update  #{_1}\n" }.join + "\nDone. Run:\n  #{native ? 'bundle exec mxrb generate project.rb' : 'cargo mxrs build'}\n"
  o.check('exact preview rendering') { out == expected }
  o.check('default target is current directory') { o.success(native, 'init', 'Sales', '--dry-run', chdir: root) == expected }
  o.check('no-progress preserves preview') { o.success(native, 'init', 'Sales', '--target', root, '--dry-run', '--no-progress') == expected }
  o.check('preview leaves all files unchanged') { o.files(root) == before }
  [payload, before]
end

Dir.mktmpdir('mxrs-presentation-oracle-') do |root|
  ruby, rust = pair(o, File.join(root, 'fresh'), workspace)
  [true, false].zip([ruby, rust]).each do |native, target|
    payload, before = preview(o, native, target)
    actual = JSON.parse(o.success(native, 'init', 'Sales', '--target', target, '--json'))
    o.check('applied result matches preview exactly') { actual == payload.merge('dry_run' => false) }
    after = o.files(target)
    created = (after.keys - before.keys).reject { _1.end_with?('/scaffolds.json') }.map { File.join(target, _1) }
    updated = (before.keys & after.keys).select { before[_1] != after[_1] }.reject { _1.end_with?('/scaffolds.json') }.map { File.join(target, _1) }
    o.check('every reported created and updated path is real') { actual['files'].sort == created.sort && actual['updated'].sort == updated.sort }
    o.readonly(target) { o.failure('init', 'Sales', '--target', target, native: false) } unless native
    if native
      out, err, status = o.run(true, 'init', 'Sales', '--target', target)
      o.check('native duplicate refused') { status != 0 && out.empty? && !err.empty? }
    end
    registry = File.join(target, native ? '.mxrb/scaffolds.json' : '.mxrs/scaffolds.json')
    o.check('registry records the scaffold') { JSON.parse(File.read(registry)).fetch('scaffolds').key?('presentation:Sales') }
    keep = File.join(target, native ? 'modules/Sales/presentation/snippets/.keep' : 'src/presentation/modules/sales/snippets/.keep')
    File.unlink(keep)
    repaired = JSON.parse(o.success(native, 'init', 'Sales', '--target', target, '--json'))
    o.check('partial initialization only restores the missing keep') { repaired['files'] == [keep] && repaired['updated'].empty? }
  end
  cache = ENV.fetch('MXRS_TEST_CARGO_TARGET_DIR') { File.join(root, 'target') }
  check_model(o, ruby, rust, cache, root)
  # The editable typed helper retains the native title/navigation options.
  native_layout = File.join(ruby, 'modules/Sales/presentation/presentation.rb')
  rust_layout = File.join(rust, 'src/presentation/modules/sales/layouts/application_layout.rs')
  File.write(native_layout, File.read(native_layout).sub('layout :ApplicationLayout', 'layout :ApplicationLayout, title: "Custom title", navigation: nil'))
  File.write(rust_layout, File.read(rust_layout).sub('application_shell("ApplicationLayout", Some("Responsive"))', 'application_shell("Custom title", None)'))
  check_model(o, ruby, rust, cache, root)

  # Explicit path mapping: each language has exactly its documented structure.
  o.check('native presentation structure') { Dir.glob(File.join(ruby, 'modules/Sales/presentation/**/*'), File::FNM_DOTMATCH).select { File.file?(_1) }.map { _1.delete_prefix(ruby + '/') }.sort == %w[modules/Sales/presentation/client_actions/.keep modules/Sales/presentation/pages/.keep modules/Sales/presentation/presentation.rb modules/Sales/presentation/snippets/.keep] }
  o.check('Rust presentation structure') { Dir.glob(File.join(rust, 'src/presentation/modules/sales/**/*'), File::FNM_DOTMATCH).select { File.file?(_1) }.map { _1.delete_prefix(File.join(rust, 'src/presentation/modules/sales/')) }.sort == %w[layouts/application_layout.rs layouts/mod.rs mod.rs nanoflows/.keep nanoflows/mod.rs pages/.keep pages/mod.rs snippets/.keep snippets/mod.rs] }
  [true, false].zip([ruby, rust]).each do |native, target|
    [['new', 'Sales'], ['init', 'Missing'], ['init', 'Sales.Bad'], ['init', '../escape']].each do |args|
      before = o.files(target)
      out, err, status = o.run(native, *args, '--target', target)
      o.check('invalid arguments fail without writing') { status != 0 && out.empty? && !err.empty? && o.files(target) == before }
    end
  end
  [[], ['init'], ['init', 'Sales', '--target', root]].each { o.failure(*_1) }
  [%w[--unknown], %w[--json --json], %w[extra]].each { o.failure('init', 'Sales', '--target', rust, *_1, native: false) }
  o.help
end
o.finish
