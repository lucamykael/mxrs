# frozen_string_literal: true
require_relative 'command_oracle_support'

# `validate` checks storage integrity: the tables, the unit tree, each unit's
# content (hash, $Type, $ID, nested $IDs, AutoNumber defaults) and the v2
# files. Both CLIs must print the same lines, on the same streams, in the
# same order, and exit alike.
o = CommandOracle.new('validate')

def same(o, *args)
  native = o.run(true, *args).map { _1.is_a?(String) ? _1.gsub('[mxrb]', '[mxrs]') : _1 }
  ours = o.run(false, *args)
  o.check("validate mismatch for #{args.inspect}: #{native.inspect} != #{ours.inspect}") { native == ours }
end

def define(path)
  FileUtils.mkdir_p(File.dirname(path))
  Mxrb.define(path) do
    mendix_version '11.12.1'
    self.module(:Sales) do
      entity(:Order) { string :Number }
      microflow(:ACT_Order_Save) {}
    end
  end
  # Mendix 11 stores each unit's content in its own v2 file.
  mpr = Mxrb::IO::MprFile.open(path, readonly: false)
  mpr.ensure_storage_for_version!('11.12.1')
  mpr.close
end

def variant(root, name, source)
  path = File.join(root, name, 'Project.mpr')
  FileUtils.mkdir_p(File.dirname(path))
  FileUtils.cp(source, path)
  FileUtils.cp_r(File.join(File.dirname(source), 'mprcontents'), File.dirname(path))
  mpr = Mxrb::IO::MprFile.open(path, readonly: false)
  yield mpr
  mpr.close
  path
end

def flow_unit(mpr)
  mpr.all_units.find { mpr.parse_contents(_1)['$Type'] == 'Microflows$Microflow' }
end

Dir.mktmpdir('mxrs-validate-oracle-') do |root|
  clean = File.join(root, 'clean', 'Project.mpr')
  define(clean)
  cases = [clean]
  cases << variant(root, 'hash', clean) do |mpr|
    mpr.query('UPDATE Unit SET ContentsHash = ? WHERE UnitID = ?',
              ['tampered', Mxrb::IO::BsonCodec.uuid_to_blob(flow_unit(mpr)['UnitID'])])
  end
  cases << variant(root, 'dangling', clean) do |mpr|
    mpr.query('UPDATE Unit SET ContainerID = ? WHERE UnitID = ?',
              [Mxrb::IO::BsonCodec.uuid_to_blob(SecureRandom.uuid),
               Mxrb::IO::BsonCodec.uuid_to_blob(flow_unit(mpr)['UnitID'])])
  end
  cases << variant(root, 'nested', clean) do |mpr|
    unit = mpr.root_unit
    doc = mpr.parse_contents(unit)
    nested = SecureRandom.uuid
    attribute = { '$ID' => nested, '$Type' => 'DomainModels$Attribute', 'Name' => 'Sequence',
                  'NewType' => { '$Type' => 'DomainModels$AutoNumberAttributeType' },
                  'Value' => { 'DefaultValue' => '0' } }
    doc['Items'] = [attribute, attribute.dup, { 'Name' => 'Late', '$ID' => SecureRandom.uuid }]
    mpr.update_unit(unit['UnitID'], doc)
  end
  cases << variant(root, 'orphan', clean) do |mpr|
    path = mpr.content_path(flow_unit(mpr))
    orphan = File.join(File.dirname(path), "#{SecureRandom.uuid}.mxunit")
    FileUtils.cp(path, orphan)
  end
  cases << variant(root, 'missing', clean) do |mpr|
    File.delete(mpr.content_path(flow_unit(mpr)))
  end
  cases << variant(root, 'legacy', clean) do |mpr|
    unit = flow_unit(mpr)
    doc = mpr.parse_contents(unit)
    content = SecureRandom.uuid
    doc['$ID'] = content
    mpr.update_unit(unit['UnitID'], doc)
    mpr.write_legacy_unit_identity_mismatches(
      [{ unit_id: unit['UnitID'], content_id: content, type: 'Microflows$Microflow' }]
    )
  end
  cases << variant(root, 'mismatch', clean) do |mpr|
    unit = flow_unit(mpr)
    doc = mpr.parse_contents(unit)
    doc['$ID'] = SecureRandom.uuid
    mpr.update_unit(unit['UnitID'], doc)
  end
  cases.each { |path| o.readonly(File.dirname(path)) { same(o, path) } }
  expected = {
    'clean' => '[mxrs] OK', 'hash' => 'ContentsHash mismatch', 'dangling' => 'references missing container',
    'nested' => 'contains duplicate nested $ID', 'orphan' => 'warning: orphan mxunit file',
    'missing' => 'missing mxunit file', 'legacy' => 'preserves legacy content $ID mismatch',
    'mismatch' => 'content $ID mismatch'
  }
  cases.each do |path|
    out, err, = o.run(false, path)
    name = File.basename(File.dirname(path))
    o.check("#{name} does not say #{expected.fetch(name)}") { (out + err).include?(expected.fetch(name)) }
  end
  o.failure
  o.failure(File.join(root, 'Absent.mpr'))
end
o.finish
