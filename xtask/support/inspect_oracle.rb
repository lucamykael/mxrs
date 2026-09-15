# frozen_string_literal: true
require_relative 'command_oracle_support'

o = CommandOracle.new('inspect', surface: 'units')
Dir.mktmpdir('mxrs-inspect-oracle-') do |root|
  %i[v1 v2].each do |format|
    path = File.join(root, "#{format}/Project with spaces.mpr")
    oracle_mpr(path, format:) do |mpr|
      parent = mpr.units_by_containment('Modules').first.fetch('UnitID')
      mpr.insert_unit(container_uuid: parent, containment_name: 'Documents', contents_doc: { '$Type' => 'Future$Type', 'Name' => 'Olá' })
      mpr.insert_unit(container_uuid: parent, containment_name: 'Documents', contents_doc: {})
    end
    # Check root Name, lowercase name, and filename fallback independently.
    %i[original lower absent empty].each do |variant|
      mpr = Mxrb::IO::MprFile.open(path, readonly: false)
      unit = mpr.root_unit
      doc = mpr.parse_contents(unit)
      doc.delete('Name')
      doc.delete('name')
      doc['Name'] = 'Nome explícito' if variant == :original
      doc['name'] = 'nome alternativo' if variant == :lower
      doc['Name'] = '' if variant == :empty
      mpr.update_unit(unit.fetch('UnitID'), doc)
      mpr.close
      o.readonly(root) do
        o.equivalent(path)
        o.equivalent(path, '--no-progress')
      end
    end
    mpr = Mxrb::IO::MprFile.open(path, readonly: false)
    unit = mpr.all_units.last
    if format == :v1
      mpr.query('UPDATE Unit SET Contents=? WHERE UnitID=?', [SQLite3::Blob.new('broken'), Mxrb::IO::BsonCodec.uuid_to_blob(unit.fetch('UnitID'))])
    else
      File.binwrite(mpr.content_path(unit), 'broken')
    end
    mpr.close
    o.readonly(root) { o.failure(path) }
  end
  empty = File.join(root, 'empty.mpr')
  oracle_mpr(empty) { |mpr| mpr.all_units.each { mpr.delete_unit(_1.fetch('UnitID')) } }
  o.readonly(root) { o.equivalent(empty) }
  [[], [root], [File.join(root, 'absent.mpr')]].each { o.readonly(root) { o.failure(*_1) } }
  [%w[--unknown], %w[extra], %w[--no-progress --no-progress]].each { o.failure(empty, *_1, native: false) }
  o.help
end
o.finish
