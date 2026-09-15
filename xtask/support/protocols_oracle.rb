# frozen_string_literal: true
require_relative 'command_oracle_support'

o = CommandOracle.new('protocols')
# GUID recognition must remain fail-closed until independently evidenced GUIDs
# enter the native registry. A changed registry must fail this oracle loudly.
o.check('native registry now needs recognized-connector fixtures') { Mxrb::Protocols.all.all? { _1.appstore_guids.empty? } }
Dir.mktmpdir('mxrs-protocols-oracle-') do |root|
  %i[v1 v2].each do |format|
    path = File.join(root, format.to_s, 'Project with spaces.mpr')
    oracle_mpr(path, format:)
    o.readonly(root) do
      o.equivalent(path)
      o.equivalent(path, '--json') { |a, b| [JSON.parse(a), JSON.parse(b)] }
    end
    mpr = Mxrb::IO::MprFile.open(path, readonly: false)
    modules = mpr.units_by_containment('Modules')
    modules.each_with_index do |unit, index|
      doc = mpr.parse_contents(unit)
      doc.merge!('Name' => index == 0 ? 'MQTT' : 'Álpha Connector', 'FromAppStore' => true,
                 'AppStoreGuid' => index == 0 ? '119508' : 'unknown-guid',
                 'AppStoreVersion' => '1.2.3', 'ExportLevel' => index == 0 ? 'Hidden' : 'Source')
      mpr.update_unit(unit.fetch('UnitID'), doc)
    end
    parent = modules.first.fetch('ContainerID')
    mpr.insert_unit(container_uuid: parent, containment_name: 'Modules', contents_doc: {
      '$Type' => 'Projects$Module', 'FromAppStore' => true, 'AppStoreGuid' => ''
    })
    mpr.close
    o.readonly(root) do
      [[], ['--no-progress']].each do |flags|
        o.equivalent(path, *flags) { |a, b| [a.gsub('`mxrb modules ', '`mxrs modules '), b] }
      end
      o.equivalent(path, '--json') { |a, b| [JSON.parse(a), JSON.parse(b)] }
      data = JSON.parse(o.success(false, path, '--json'))
      o.check('name/component-id/empty GUID must not identify a connector') do
        data['connectors'] == [] && data['unknown_marketplace_modules'] == ['', 'MQTT', 'Álpha Connector']
      end
    end
    [%w[--unknown], %w[extra], %w[--json --json]].each { o.failure(path, *_1, native: false) }
  end
  corrupt = File.join(root, 'corrupt.mpr')
  File.binwrite(corrupt, 'not SQLite')
  [[], [root], [corrupt], [File.join(root, 'absent.mpr')]].each { o.readonly(root) { o.failure(*_1) } }
  o.help
end
o.finish
