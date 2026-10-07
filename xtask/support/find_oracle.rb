# frozen_string_literal: true
require_relative 'command_oracle_support'

# `find` by name is a case-insensitive match on qualified names, listed by
# module, kind and name; `--semantic` is the ten a TF-IDF search ranks first.
o = CommandOracle.new('find')

def fixture(path)
  FileUtils.mkdir_p(File.dirname(path))
  Mxrb.define(path) do
    mendix_version '11.12.1'
    self.module(:Zulu) do
      entity(:Record) { string :Name; association :Other, name: :Record_Other }
      entity(:Other) {}
      microflow(:Target) {}
      microflow(:Caller) { call microflow: 'Zulu.Target' }
      nanoflow(:Client) {}
      page(:Shared) { title 'Shared' }
      microflow(:Shared) {}
    end
    self.module(:Alpha) do
      entity(:Ledger) { string :Total }
      microflow(:ACT_Ledger_Close) {}
    end
  end
end

Dir.mktmpdir('mxrs-find-oracle-') do |root|
  path = File.join(root, 'Project.mpr')
  fixture(path)
  o.readonly(root) do
    ['zulu', 'ZULU.', 'target', 'Shared', 'record_other', 'ledger', '.', 'missing'].each do |text|
      o.equivalent(path, text)
    end
    ['zulu target', 'ledger close microflow', 'Shared page'].each do |text|
      o.equivalent(path, text, '--semantic')
    end
  end
  o.failure(path)
  o.failure(File.join(root, 'Missing.mpr'), 'zulu')
end
o.finish
