from pathlib import Path
import re
source=Path('src/metrics.rs').read_text()
macro='''// Build immutable exposition text at compile time; only values need formatting.
macro_rules! sample {
    ($name:literal, $kind:literal, $value:expr, $help:literal $(,)?) => {
        (concat!("# HELP war_", $name, " ", $help,
                 "\\n# TYPE war_", $name, " ", $kind,
                 "\\nwar_", $name, " "), $value)
    };
}

'''
source=source.replace('impl Metrics {',macro+'impl Metrics {',1)
pattern=r'        for \(([^)]+)\) in \[(.*?)\n        \] \{(.*?)\n        \}'
index=0
rows_total=0
def convert(match):
 global index,rows_total
 rows=re.findall(r'\(\s*("[a-z_]+"),\s*(?:("(?:gauge|counter)"),\s*)?(.*?),\s*("[^"\n]*"),\s*\)',match[2],re.S)
 assert len(rows)==[4,30,9][index]
 text='        for (prefix, value) in [\n'
 for name,kind,value,helptext in rows:
  kind=kind or ('"counter"' if index==1 else '"gauge"')
  text+=f'            sample!({name}, {kind}, {value}, {helptext}),\n'
 text+='''        ] {
            text.push_str(prefix);
            writeln!(text, "{value}").expect("String write");
        }'''
 rows_total+=len(rows);index+=1;return text
source=re.sub(pattern,convert,source,flags=re.S);assert index==3 and rows_total==43
source=source.replace('''        // Current exposition fits within 8 KiB even with 20-digit values.
        // This is a capacity hint, not a limit if metrics are added later.''','''        // Reserve for the common exposition. Wider values or future metrics
        // can grow the buffer normally; this is a capacity hint, not a limit.''')
Path('artifacts/metrics-static-20261019-selected.rs').write_text(source)
