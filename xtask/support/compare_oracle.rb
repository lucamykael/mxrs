# frozen_string_literal: true
require_relative 'command_oracle_support'

o = CommandOracle.new(defined?(ORACLE_COMMAND) ? ORACLE_COMMAND : 'compare')

def snapshot(path)
  # Native snapshot facts are also checked through the actual compare CLI's
  # complete differences below. Ruby BSON values use their extended JSON form.
  JSON.parse(JSON.generate(Mxrb::Compare::Comparator.new(path, path).send(:snapshot, path)))
end

def json_pattern(value, numbers, root: false)
  case value
  when nil then root ? 'nil' : 'null'
  when Float
    numbers << value
    '([-+0-9.eE]+)'
  when Array then '\[' + value.map { json_pattern(_1, numbers) }.join(',') + '\]'
  when Hash then '\{' + value.keys.sort.map { Regexp.escape(JSON.generate(_1)) + ':' + json_pattern(value[_1], numbers) }.join(',') + '\}'
  else Regexp.escape(JSON.generate(value))
  end
end

def compare_pair(o, left, right)
  expected = Mxrb.compare(left, right)
  native, errors, status = o.run(true, left, right)
  native_text = if o.command == 'diff'
    expected.changes.map { "#{_1.operation}\t#{_1.path.join('.')}\t#{_1.before.inspect}\t=>\t#{_1.after.inspect}\n" }.join
  else
    expected.identical? ? "[mxrb] OK\n" : expected.differences.map { "[mxrb] diff: #{_1}\n" }.join
  end
  o.check('native CLI exact differences and status') { errors.empty? && status == (expected.identical? ? 0 : 1) && native == native_text }
  changes = JSON.parse(JSON.generate(expected.changes.map do |change|
    { operation: change.operation.to_s.capitalize,
      path: change.path.map { _1.is_a?(Integer) ? _1 : _1.to_s }, before: change.before, after: change.after }
  end))
  out, errors, status = o.run(false, left, right, '--json')
  unless JSON.parse(out) == { 'identical' => expected.identical?, 'changes' => changes }
    warn "expected changes: #{changes.inspect}\nactual: #{out}"
  end
  o.check('complete ordered differences') do
    errors.empty? && status == (expected.identical? ? 0 : 1) && JSON.parse(out) == { 'identical' => expected.identical?, 'changes' => changes }
  end
  [[], ['--no-progress']].each do |flags|
    out, errors, status = o.run(false, left, right, *flags)
    numbers = []
    pattern = if expected.identical?
      o.command == 'diff' ? '' : Regexp.escape("[mxrs] OK\n")
    else
      changes.map do |change|
        before = json_pattern(change['before'], numbers, root: true)
        after = json_pattern(change['after'], numbers, root: true)
        if o.command == 'diff'
          Regexp.escape("#{change['operation'].downcase}\t#{change['path'].join('.')}\t") + before + Regexp.escape("\t=>\t") + after + "\n"
        else
          path = change['path'].map { _1.is_a?(Integer) ? "[#{_1}]" : _1 }.join('.')
          Regexp.escape("[mxrs] diff: #{path}: ") + before + Regexp.escape(' != ') + after + "\n"
        end
      end.join
    end
    match = Regexp.new("\\A#{pattern}\\z").match(out)
    o.check('human differences and exit status') do
      errors.empty? && status == (expected.identical? ? 0 : 1) && match && match.captures.zip(numbers).all? { |actual, value| Float(actual) == value }
    end
  end
end

def compare_fixture(path)
  FileUtils.mkdir_p(File.dirname(path))
  Mxrb.define(path) do
    mendix_version '11.12.1'
    self.module(:Zulu) do
      module_role :Reader, description: 'Read data'
      module_role :Writer, description: 'Write data'
      entity(:Record) do
        string :Name, documentation: 'Record name'
        decimal :Total, default: 0
        association :Other, name: 'Record_Other'
        access_rule 'Zulu.Reader', read: :all
      end
      entity(:Other) {}
      page(:Overview) do
        title 'Records'
        text :hint, caption: 'Hello'
      end
      menu(:Navigation) { item 'Overview', page: 'Zulu.Overview' }
      microflow(:Work) do
        parameter :Flag, type: :Boolean
        allowed_roles 'Zulu.Reader', 'Zulu.Writer'
        decision '$Flag' do
          on(true) { log_message 'yes' }
          on(false) { log_message 'no' }
        end
      end
      nanoflow(:Client) {}
    end
    self.module(:Alpha) {}
  end
end

def walk_docs(value, &block)
  case value
  when Hash
    yield value
    value.each_value { walk_docs(_1, &block) }
  when Array then value.each { walk_docs(_1, &block) }
  end
end

def edit(path, type, name = nil)
  mpr = Mxrb::IO::MprFile.open(path, readonly: false)
  unit = mpr.all_units.find do |candidate|
    doc = mpr.parse_contents(candidate)
    doc['$Type'] == type && (!name || (doc['Name'] || doc['name']) == name)
  end
  raise "missing fixture unit #{type}/#{name}" unless unit
  doc = mpr.parse_contents(unit)
  yield doc
  mpr.update_unit(unit.fetch('UnitID'), doc)
ensure
  mpr&.close
end

def nested(document, type, name = nil)
  matches = []
  walk_docs(document) { |doc| matches << doc if doc['$Type'] == type && (!name || (doc['Name'] || doc['name']) == name) }
  if matches.empty?
    types = []
    walk_docs(document) { types << [_1['$Type'], _1['Name'] || _1['name']] }
    raise "missing #{type}/#{name}: #{types.uniq.inspect}"
  end
  matches.first
end

def key(document, *keys)
  keys.find { document.key?(_1) } || raise("missing native field #{keys.inspect}")
end

def check_snapshot(o, path)
  out, errors, status = Open3.capture3(o.mxrs, 'inspect', path, '--json')
  actual = JSON.parse(out) if status.success?
  expected = snapshot(path)
  unless actual == expected
    mismatch = Mxrb::Compare::Comparator.new(path, path).send(:diff_values, expected, actual)
    warn "snapshot mismatch: #{mismatch.first(10).inspect}"
  end
  o.check("full snapshot: #{errors}") { status.success? && errors.empty? && actual == expected }
end

Dir.mktmpdir('mxrs-compare-oracle-') do |root|
  left = File.join(root, 'left/Project.mpr')
  compare_fixture(left)
  o.readonly(root) { check_snapshot(o, left); compare_pair(o, left, left) }
  mutations = {
    'attribute_length' => lambda { |path|
      edit(path, 'DomainModels$DomainModel') do |doc|
        attribute = nested(doc, 'DomainModels$Attribute', 'Name')
        type = attribute.fetch(key(attribute, 'Type', 'type', 'NewType', 'newType'))
        type[key(type, 'Length', 'length')] = 333
      end
    },
    'attribute_default' => lambda { |path|
      edit(path, 'DomainModels$DomainModel') do |doc|
        attribute = nested(doc, 'DomainModels$Attribute', 'Total')
        value = attribute.fetch(key(attribute, 'Value', 'value'))
        value[key(value, 'DefaultValue', 'defaultValue')] = '42'
      end
    },
    'numeric_default' => lambda { |path|
      edit(path, 'DomainModels$DomainModel') do |doc|
        value = nested(doc, 'DomainModels$Attribute', 'Total').then { _1.fetch(key(_1, 'Value', 'value')) }
        value[key(value, 'DefaultValue', 'defaultValue')] = 7
      end
    },
    'float_default' => lambda { |path|
      edit(path, 'DomainModels$DomainModel') do |doc|
        value = nested(doc, 'DomainModels$Attribute', 'Total').then { _1.fetch(key(_1, 'Value', 'value')) }
        value[key(value, 'DefaultValue', 'defaultValue')] = 1.25e20
      end
    },
    'generalization' => lambda { |path|
      edit(path, 'DomainModels$DomainModel') do |doc|
        entity = nested(doc, 'DomainModels$EntityImpl', 'Record')
        generalization = entity.fetch(key(entity, 'Generalization', 'generalization', 'MaybeGeneralization'))
        generalization['HasOwnerAttr'] = true
      end
    },
    'access_rule' => lambda { |path|
      edit(path, 'DomainModels$DomainModel') { nested(_1, 'DomainModels$AccessRule')['XPathConstraint'] = '[Name != empty]' }
    },
    'delete_behavior' => lambda { |path|
      edit(path, 'DomainModels$DomainModel') do |doc|
        association = nested(doc, 'DomainModels$Association', 'Record_Other')
        behavior = association.fetch(key(association, 'DeleteBehavior', 'deleteBehavior'))
        behavior[key(behavior, 'ParentDeleteBehavior', 'parentDeleteBehavior')] = 'Delete'
      end
    },
    'module_role' => lambda { |path|
      edit(path, 'Security$ModuleSecurity') { nested(_1, 'Security$ModuleRole', 'Reader')['Description'] = 'Changed role' }
    },
    'flow_case' => lambda { |path|
      edit(path, 'Microflows$Microflow', 'Work') do |doc|
        found = false
        walk_docs(doc) do |value|
          if value['$Type']&.end_with?('$EnumerationCase') && value['Value'] == 'true'
            value['Value'] = 'changed'; found = true
          end
        end
        raise 'case mutation did not change anything' unless found
      end
    },
    'flow_permissions' => lambda { |path|
      edit(path, 'Microflows$Microflow', 'Work') { _1['AllowedModuleRoles'] = [1, 'Zulu.Writer'] }
    },
    'nanoflow_return' => lambda { |path|
      edit(path, 'Microflows$Nanoflow', 'Client') { _1['MicroflowReturnType'] = { '$Type' => 'DataTypes$StringType' } }
    },
    'security' => lambda { |path|
      edit(path, 'Security$ProjectSecurity') { _1['AdminUserName'] = 'alternate' }
    },
    'asset' => lambda { |path|
      FileUtils.mkdir_p(File.join(File.dirname(path), 'theme'))
      File.binwrite(File.join(File.dirname(path), 'theme/file.css'), 'body { color: red; }')
    },
    'backslash_asset' => lambda { |path|
      directory = File.join(File.dirname(path), 'theme')
      FileUtils.mkdir_p(File.join(directory, 'sub'))
      File.write(File.join(directory, 'sub/file.css'), 'nested')
      File.write(File.join(directory, 'sub\\file.css'), 'literal backslash')
    },
    'page_title' => lambda { |path|
      edit(path, 'Forms$Page', 'Overview') do |doc|
        walk_docs(doc) { _1['Text'] = 'Changed title' if _1['Text'] == 'Records' }
      end
    },
    'page_widget' => lambda { |path|
      edit(path, 'Forms$Page', 'Overview') do |doc|
        walk_docs(doc) { _1['Text'] = 'Changed widget' if _1['Text'] == 'Hello' }
      end
    },
    'page_permissions' => lambda { |path|
      edit(path, 'Forms$Page', 'Overview') { _1['AllowedModuleRoles'] = [1, 'Zulu.Reader'] }
    },
    'menu' => lambda { |path|
      edit(path, 'Menus$MenuDocument', 'Navigation') do |doc|
        walk_docs(doc) { _1['Text'] = 'New caption' if _1['Text'] == 'Overview' }
      end
    },
    'navigation' => lambda { |path|
      edit(path, 'Navigation$NavigationDocument') do |doc|
        doc['Profiles'] = [3, { '$Type' => 'Navigation$NavigationProfile', 'Name' => 'Phone', 'Kind' => 'OfflinePhone',
          'AppTitle' => { 'Translations' => [3, { 'LanguageCode' => 'en_US', 'Text' => 'Mobile app' }] },
          'HomePage' => { 'Page' => 'Zulu.Overview', 'Microflow' => 'Zulu.Work' } }]
      end
    },
    'entity_rename' => lambda { |path|
      edit(path, 'DomainModels$DomainModel') do |doc|
        entity = nested(doc, 'DomainModels$EntityImpl', 'Record')
        entity[key(entity, 'Name', 'name')] = 'Renamed'
      end
    },
    'unknown_unit' => lambda { |path|
      mpr = Mxrb::IO::MprFile.open(path, readonly: false)
      parent = mpr.units_by_containment('Modules').first.fetch('UnitID')
      mpr.insert_unit(container_uuid: parent, containment_name: 'Documents', contents_doc: { '$Type' => 'Future$Document', 'Name' => 'Future' })
      mpr.close
    },
    'flow_reorder' => lambda { |path|
      edit(path, 'Microflows$Microflow', 'Work') do |doc|
        doc['ObjectCollection']['Objects'][1..] = doc['ObjectCollection']['Objects'][1..].reverse
        doc['Flows'][1..] = doc['Flows'][1..].reverse
      end
    },
    'flow_layout' => lambda { |path|
      edit(path, 'Microflows$Microflow', 'Work') do |doc|
        walk_docs(doc['ObjectCollection']) do |value|
          value['RelativeMiddlePoint'] = '300;400' if value.key?('RelativeMiddlePoint') && value['$Type'] != 'Microflows$MicroflowParameter'
        end
      end
    },
    'hidden_asset' => lambda { |path|
      directory = File.join(File.dirname(path), 'theme/.private')
      FileUtils.mkdir_p(directory)
      File.write(File.join(directory, 'ignored.css'), 'hidden')
    },
    'v2' => lambda { |path|
      mpr = Mxrb::IO::MprFile.open(path, readonly: false)
      mpr.ensure_storage_for_version!('11.12.1')
      mpr.close
    }
  }
  mutations.each do |name, mutation|
    right = File.join(root, name, 'Project.mpr')
    FileUtils.mkdir_p(File.dirname(right))
    FileUtils.cp(left, right)
    mutation.call(right)
    expect_equal = %w[flow_reorder flow_layout hidden_asset].include?(name)
    o.check("fixture mutation #{name} must #{expect_equal ? 'preserve' : 'change'} native meaning") { Mxrb.compare(left, right).identical? == expect_equal }
    o.readonly(root) do
      check_snapshot(o, right)
      compare_pair(o, left, right)
      compare_pair(o, right, left)
    end
    puts "PASS #{o.command} #{name}"
  end
  numeric_paths = %w[integer double].map do |kind|
    path = File.join(root, kind, 'Project.mpr')
    FileUtils.mkdir_p(File.dirname(path))
    FileUtils.cp(left, path)
    edit(path, 'DomainModels$DomainModel') do |doc|
      value = nested(doc, 'DomainModels$Attribute', 'Total').then { _1.fetch(key(_1, 'Value', 'value')) }
      value[key(value, 'DefaultValue', 'defaultValue')] = kind == 'integer' ? 1 : 1.0
    end
    path
  end
  o.readonly(root) { compare_pair(o, *numeric_paths) }
  # Intentional stability improvement retained from MXRS: equal flow names are
  # ordered by normalized content. Native name-only sorting exposes row order.
  duplicate_left = File.join(root, 'duplicates-left/Project.mpr')
  duplicate_right = File.join(root, 'duplicates-right/Project.mpr')
  FileUtils.mkdir_p(File.dirname(duplicate_left))
  FileUtils.mkdir_p(File.dirname(duplicate_right))
  Mxrb.define(duplicate_left) do
    mendix_version '11.12.1'
    self.module(:Calls) do
      microflow(:First) { log_message 'one' }
      microflow(:Second) { log_message 'two' }
    end
  end
  %w[First Second].each { |name| edit(duplicate_left, 'Microflows$Microflow', name) { _1['Name'] = 'Duplicate' } }
  FileUtils.cp(duplicate_left, duplicate_right)
  mpr = Mxrb::IO::MprFile.open(duplicate_right, readonly: false)
  mpr.query('CREATE TEMP TABLE reversed_units AS SELECT * FROM Unit ORDER BY rowid DESC')
  mpr.query('DELETE FROM Unit')
  mpr.query('INSERT INTO Unit SELECT * FROM reversed_units ORDER BY rowid')
  mpr.close
  o.readonly(root) do
    out, err, status = o.run(true, duplicate_left, duplicate_right)
    o.check('native duplicate-name row ordering is observed explicitly') { status == 1 && err.empty? && !out.empty? }
    [[], ['--json']].each do |flags|
      out = o.success(false, duplicate_left, duplicate_right, *flags)
      expected = if flags.empty?
        o.command == 'compare' ? "[mxrs] OK\n" : ''
      else
        { 'identical' => true, 'changes' => [] }
      end
      o.check('duplicate-name reordering is semantically stable in MXRS') { (flags.empty? ? out : JSON.parse(out)) == expected }
    end
  end
  edit(duplicate_right, 'Microflows$Microflow', 'Duplicate') do |doc|
    walk_docs(doc) { |value| value.each { |key, text| value[key] = 'changed' if %w[one two].include?(text) } }
  end
  o.readonly(root) do
    out, err, status = o.run(false, duplicate_left, duplicate_right, '--json')
    o.check('duplicate ordering cannot hide a real body edit') { status == 1 && err.empty? && !JSON.parse(out)['changes'].empty? }
  end
  # A path expected to be an asset directory cannot silently become an empty
  # inventory. MXRS refuses this more strictly than native glob enumeration.
  broken_assets = File.join(root, 'broken-assets/Project.mpr')
  FileUtils.mkdir_p(File.dirname(broken_assets))
  FileUtils.cp(left, broken_assets)
  File.write(File.join(File.dirname(broken_assets), 'theme'), 'not a directory')
  o.readonly(root) { o.failure(left, broken_assets, native: false) }
  invalid_path = File.join(root, 'invalid-encoding/Project.mpr')
  FileUtils.mkdir_p(File.join(File.dirname(invalid_path), 'theme'))
  FileUtils.cp(left, invalid_path)
  File.binwrite(File.join(File.dirname(invalid_path), 'theme').b + "/\xff.css".b, 'data')
  o.readonly(root) { o.failure(left, invalid_path, native: false) }
  complex_default = File.join(root, 'complex-default/Project.mpr')
  FileUtils.mkdir_p(File.dirname(complex_default))
  FileUtils.cp(left, complex_default)
  edit(complex_default, 'DomainModels$DomainModel') do |doc|
    value = nested(doc, 'DomainModels$Attribute', 'Total').then { _1.fetch(key(_1, 'Value', 'value')) }
    value[key(value, 'DefaultValue', 'defaultValue')] = { 'unexpected' => 'object' }
  end
  o.readonly(root) { o.failure(left, complex_default, native: false) }
  bad = File.join(root, 'bad.mpr')
  File.write(bad, 'not SQLite')
  [[], [left], [left, bad], [File.join(root, 'absent.mpr'), left], [root, left]].each { o.readonly(root) { o.failure(*_1) } }
  [%w[--unknown], %w[--json --json], %w[extra]].each { o.failure(left, left, *_1, native: false) }
  o.help
end
o.finish
