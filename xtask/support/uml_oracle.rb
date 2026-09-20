# frozen_string_literal: true

# Differential oracle for `uml --export`: both CLIs must print byte-identical
# Mermaid/PlantUML text for class, activity, and sequence diagrams over the
# same disposable native models, and must reject the same invalid inputs.
# The interactive viewer is deliberately NOT exercised: MXRB would block
# serving HTTP, and MXRS refuses viewer mode explicitly (checked below).
require_relative 'command_oracle_support'

o = CommandOracle.new('uml')

def fixture(path)
  FileUtils.mkdir_p(File.dirname(path))
  Mxrb.define(path) do
    mendix_version '11.12.1'
    self.module(:Zulu) do
      enumeration(:Status) { value :Open; value :Closed }
      entity(:Order) do
        string :Number
        integer :Total
        enum :Status, enumeration: 'Zulu.Status'
        association :Customer, name: :Order_Customer
      end
      entity(:Customer) { string :Name }
      microflow(:Target) {}
      microflow(:Helper) { call microflow: 'Zulu.Target' }
      microflow(:Caller) do
        call microflow: 'Zulu.Helper'
        call microflow: 'Zulu.Target'
      end
      nanoflow(:Client) {}
    end
    self.module(:Alpha) do
      entity(:Lead) { string :Email }
      microflow(:Isolated) {}
    end
  end
end

Dir.mktmpdir('mxrs-uml-oracle-') do |root|
  v1 = File.join(root, 'v1', 'Uml Fixture.mpr')
  fixture(v1)
  v2 = File.join(root, 'v2', 'Uml Fixture.mpr')
  fixture(v2)
  mpr = Mxrb::IO::MprFile.open(v2, readonly: false)
  mpr.ensure_storage_for_version!('11.12.1')
  mpr.close

  [v1, v2].each do |path|
    o.readonly(root) do
      %w[mermaid plantuml].each do |format|
        o.equivalent(path, '--export', 'class', '--format', format)
        o.equivalent(path, '--export', 'class', '--module', 'Zulu', '--format', format)
        o.equivalent(path, '--export', 'activity', '--microflow', 'Zulu.Caller', '--format', format)
        o.equivalent(path, '--export', 'activity', '--microflow', 'Zulu.Target', '--format', format)
        o.equivalent(path, '--export', 'sequence', '--root', 'Zulu.Caller', '--format', format)
        o.equivalent(path, '--export', 'sequence', '--module', 'Zulu', '--format', format)
      end
      # Defaults: mermaid format, depth 2; explicit depths change expansion.
      o.equivalent(path, '--export', 'class')
      o.equivalent(path, '--export', 'sequence', '--root', 'Zulu.Caller', '--depth', '1')
      o.equivalent(path, '--export', 'sequence', '--root', 'Zulu.Caller', '--depth', '3')
      o.equivalent(path, '--export', 'sequence', '--module', 'Alpha') # no call edges
    end
  end

  # Shared refusals: same invalid input must fail in both implementations.
  o.failure(v1, '--export', 'bogus')
  o.failure(v1, '--export', 'class', '--format', 'bogus')
  o.failure(v1, '--export', 'class', '--module', 'Missing')
  o.failure(v1, '--export', 'activity')
  o.failure(v1, '--export', 'activity', '--microflow', 'Zulu.Missing')
  o.failure(v1, '--export', 'sequence')
  o.failure(v1, '--export', 'sequence', '--root', 'Zulu.Caller', '--module', 'Zulu')
  o.failure(v1, '--export', 'sequence', '--root', 'Zulu.Missing')
  o.failure(v1, '--export', 'sequence', '--module', 'Missing')
  o.failure(v1, '--export', 'sequence', '--root', 'Zulu.Caller', '--depth', '-1')
  o.failure(v1, '--export', 'sequence', '--root', 'Zulu.Caller', '--depth', '101')
  o.failure(File.join(root, 'missing.mpr'), '--export', 'class')
  o.failure # no source at all

  # MXRS-only: viewer mode is not ported and must refuse, not hang or serve.
  o.failure(v1, native: false)
  out, err, code = o.run(false, v1)
  o.check('viewer refusal names --export') { code != 0 && out.empty? && err.include?('--export') }

  o.help
end
o.finish
