# frozen_string_literal: true
require_relative 'command_oracle_support'

o = CommandOracle.new(ORACLE_COMMAND)

def artifacts_json(artifacts)
  keys = Mxrb::Semantic::Index::CACHE_METADATA_KEYS
  JSON.parse(JSON.generate(artifacts.map do |a|
    { id: a.id, qualified_name: a.qualified_name, kind: a.kind, module_name: a.module_name,
      name: a.name, unit_id: a.unit_id, path: a.path, metadata: a.metadata.slice(*keys) }
  end))
end

def references_json(references)
  JSON.parse(JSON.generate(references.map do |r|
    { from: r.source.id, to: r.target.id, relation: r.relation, path: r.path, value: r.value }
  end))
end

def tree_json(index, selected)
  artifacts = index.artifacts.select { !selected || _1.module_name == selected }
  artifacts.group_by(&:module_name).to_h do |mod, items|
    [mod || '(project)', items.reject { _1.kind == :module }.group_by(&:kind).to_h { |kind, grouped| [kind.to_s, grouped.map(&:qualified_name).sort] }]
  end
end

def verify(o, path, name = nil)
  args = [path, name].compact
  native = o.success(true, *args)
  o.check('native human output is byte-identical') { o.success(false, *args) == native }
  o.check('no-progress preserves human output') { o.success(false, *args, '--no-progress') == native }
  actual = JSON.parse(o.success(false, *args, '--json'))
  Mxrb.open(path) do |project|
    index = project.semantic_index
    expected = case o.command
    when 'callers' then artifacts_json(index.callers_of(name))
    when 'callees' then artifacts_json(index.callees_of(name))
    when 'impact' then artifacts_json(index.impact_of(name).artifacts)
    when 'refs'
      { 'artifact' => name, 'incoming' => references_json(index.references_to(name)) }
    when 'describe'
      details = index.describe(name)
      { 'artifact' => artifacts_json([details.artifact]).first,
        'incoming' => references_json(details.incoming), 'outgoing' => references_json(details.outgoing) }
    when 'tree' then tree_json(index, name)
    end
    if %w[describe refs].include?(o.command)
      fingerprint = actual.delete('fingerprint')
      o.check('document fingerprint') { fingerprint.match?(/\A[0-9a-f]{64}\z/) }
    end
    o.check("complete structured facts: #{expected.inspect} != #{actual.inspect}") { actual == expected }
  end
end

def edit_unit(mpr, type, name = nil)
  unit = mpr.all_units.find do |unit|
    doc = mpr.parse_contents(unit)
    doc['$Type'] == type && (!name || (doc['Name'] || doc['name']) == name)
  end or raise "missing #{type}/#{name}"
  doc = mpr.parse_contents(unit)
  yield doc
  mpr.update_unit(unit['UnitID'], doc)
end

def fixture(path)
  FileUtils.mkdir_p(File.dirname(path))
  Mxrb.define(path) do
    mendix_version '11.12.1'
    self.module(:Zulu) do
      entity(:Record) { string :Name; association :Other, name: :Record_Other }
      entity(:Other) {}
      microflow(:Target) {}
      microflow(:Unused) {}
      microflow(:Caller) { call microflow: 'Zulu.Target'; call microflow: 'Zulu.Target' }
      nanoflow(:Client) {}
      page(:Shared) { title 'Shared' }
      microflow(:Shared) {}
    end
    self.module(:Alpha) {}
  end
  mpr = Mxrb::IO::MprFile.open(path, readonly: false)
  parent = mpr.units_by_containment('Modules').find { mpr.parse_contents(_1)['Name'] == 'Zulu' }['UnitID']
  root = mpr.root_unit['UnitID']
  folder = mpr.insert_unit(container_uuid: parent, containment_name: 'Folders', contents_doc: { '$Type' => 'Projects$Folder', 'Name' => 'Nested' })
  unknown = mpr.insert_unit(container_uuid: folder, containment_name: 'Documents', contents_doc: {
    '$Type' => 'Future$ExperimentalDocument', 'Name' => 'Future',
    'Documentation' => 'Zulu.Unused', 'Caption' => 'Zulu.Unused',
    'Excluded' => true, 'MarkAsUsed' => true, 'Published' => 'Yes',
    'Refs' => [1, { 'Microflow' => 'Zulu.Target' }, { 'Microflow' => 'Zulu.Target' },
                { 'Microflow' => 'Target' }, { 'Expression' => '$record/Zulu.Record/Name + Zulu.Target' },
                { 'Page' => 'Zulu.Shared' }, { 'Microflow' => 'Zulu.Shared' },
                { 'Microflow' => 'Zulu.DoesNotExist' }, { 'Microflow' => 'zulu.Target' }],
    'AllowedRoles' => [1, 'Zulu.Reader'], 'documentation' => 'lowercase metadata is scanned natively: Zulu.Target'
  })
  mpr.insert_unit(container_uuid: root, containment_name: 'Documents', contents_doc: { '$Type' => 'Future$HTTPConnector', 'name' => 'Global', 'Microflow' => 'Zulu.Target' })
  mpr.insert_unit(container_uuid: folder, containment_name: 'Documents', contents_doc: { '$Type' => 'JavaScriptActions$JavaScriptAction', 'Name' => 'Script', 'Microflow' => 'Zulu.Caller' })
  mpr.insert_unit(container_uuid: folder, containment_name: 'Documents', contents_doc: { '$Type' => 'Future$Nameless', 'Microflow' => 'Zulu.Target' })
  edit_unit(mpr, 'Microflows$Microflow', 'Target') { _1['Extra'] = { 'Microflow' => 'Zulu.Caller' } }
  mpr.close
  [parent, unknown]
end

Dir.mktmpdir('mxrs-documents-oracle-') do |root|
  path = File.join(root, 'v1/Project.mpr')
  parent, unknown = fixture(path)
  queries = o.command == 'tree' ? [nil, 'Zulu', 'Alpha', 'Missing', 'zulu'] : %w[Zulu.Target Zulu.Caller Zulu.Future Zulu.Record Zulu.Record.Name Zulu.Record_Other Zulu.Other Zulu.Unused Zulu.Script Global Project.Navigation Alpha]
  o.readonly(root) { queries.each { verify(o, path, _1) } }
  v2 = File.join(root, 'v2/Project.mpr')
  FileUtils.mkdir_p(File.dirname(v2)); FileUtils.cp(path, v2)
  mpr = Mxrb::IO::MprFile.open(v2, readonly: false)
  mpr.ensure_storage_for_version!('11.12.1'); mpr.close
  o.readonly(root) { verify(o, v2, o.command == 'tree' ? nil : 'Zulu.Target') }

  # Reordering storage changes encounter order. Compare the actual output in
  # that order instead of sorting it into agreement.
  reordered = File.join(root, 'reordered/Project.mpr')
  FileUtils.mkdir_p(File.dirname(reordered)); FileUtils.cp(path, reordered)
  mpr = Mxrb::IO::MprFile.open(reordered, readonly: false)
  mpr.query('CREATE TEMP TABLE Reordered AS SELECT * FROM Unit ORDER BY rowid DESC')
  mpr.query('DELETE FROM Unit'); mpr.query('INSERT INTO Unit SELECT * FROM Reordered'); mpr.close
  o.readonly(root) { verify(o, reordered, o.command == 'tree' ? nil : 'Zulu.Target') }

  if o.command != 'tree'
    ['Zulu.Shared', 'Missing', 'Target', 'zulu.Target', unknown, "unit:#{unknown}"].each { |name| o.readonly(root) { o.failure(path, name) } }
  end
  # Cyclic containment must terminate and classify the stranded document as
  # project-level. No invented module ancestor is allowed.
  cyclic = File.join(root, 'cyclic/Project.mpr')
  FileUtils.mkdir_p(File.dirname(cyclic)); FileUtils.cp(path, cyclic)
  mpr = Mxrb::IO::MprFile.open(cyclic, readonly: false)
  blob = Mxrb::IO::BsonCodec.uuid_to_blob(unknown)
  mpr.query('UPDATE Unit SET ContainerID = ? WHERE UnitID = ?', [blob, blob]); mpr.close
  o.readonly(root) { verify(o, cyclic, o.command == 'tree' ? nil : 'Future') }

  unnamed = File.join(root, 'unnamed/Project.mpr')
  FileUtils.mkdir_p(File.dirname(unnamed)); FileUtils.cp(path, unnamed)
  mpr = Mxrb::IO::MprFile.open(unnamed, readonly: false)
  unit = mpr.units_by_containment('Modules').find { mpr.parse_contents(_1)['Name'] == 'Alpha' }
  doc = mpr.parse_contents(unit); doc.delete('Name'); mpr.update_unit(unit['UnitID'], doc)
  mpr.close
  o.readonly(root) { verify(o, unnamed, o.command == 'tree' ? nil : 'Zulu.Target') }
  corrupted = File.join(root, 'corrupt/Project.mpr')
  FileUtils.mkdir_p(File.dirname(corrupted)); FileUtils.cp(path, corrupted)
  mpr = Mxrb::IO::MprFile.open(corrupted, readonly: false)
  mpr.query('UPDATE Unit SET Contents=? WHERE UnitID=?', [SQLite3::Blob.new('broken'), Mxrb::IO::BsonCodec.uuid_to_blob(unknown)]); mpr.close
  o.readonly(root) { o.failure(corrupted, *(o.command == 'tree' ? [] : ['Zulu.Target'])) }
  empty = File.join(root, 'empty/Project.mpr')
  oracle_mpr(empty) { |mpr| mpr.all_units.each { mpr.delete_unit(_1.fetch('UnitID')) } }
  o.readonly(root) do
    o.command == 'tree' ? verify(o, empty) : o.failure(empty, 'Zulu.Target')
  end
  bad = File.join(root, 'bad.mpr'); File.write(bad, 'not SQLite')
  args = o.command == 'tree' ? [] : ['Zulu.Target']
  [[], [File.join(root, 'missing.mpr'), *args], [bad, *args], [root, *args]].each { o.failure(*_1) }
  [%w[--unknown], %w[--json --json], %w[extra surplus]].each { o.failure(path, *args, *_1, native: false) }
  o.help
end
o.finish
