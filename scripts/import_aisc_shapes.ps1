# Converts the AISC Shapes Database v16.0 spreadsheet into the section library
# bundled with oa-model, crates/oa-model/data/aisc_v16.json.
#
#   .\scripts\import_aisc_shapes.ps1 -Workbook path\to\aisc-shapes-database-v160-2.xlsx
#
# The workbook is AISC's download from
# https://www.aisc.org/aisc/publications/steel-construction-manual/aisc-shapes-database-v160/
# and is not kept in the repository. Only the US customary columns are read.
#
# Single angles (L) are left out because they bend about principal axes
# inclined to the legs, which a frame section's two second moments cannot
# describe. Double angles (2L) are left out because the table gives them no
# torsion constant, which every frame section needs.
param(
    [Parameter(Mandatory = $true)][string]$Workbook,
    [string]$Out = (Join-Path (Split-Path $PSScriptRoot -Parent) 'crates/oa-model/data/aisc_v16.json')
)
$ErrorActionPreference = 'Stop'

$kinds = @('W', 'M', 'S', 'HP', 'C', 'MC', 'WT', 'MT', 'ST', 'HSS', 'PIPE')
# Spreadsheet column to property name. Area, Ix, Iy and J become the
# section's own area, iz, iy and torsion.
$base = [ordered]@{ area = 'F'; iy = 'AQ'; iz = 'AM'; torsion = 'AX' }
# Name and column pairs; a list because names such as b and B differ only
# in case, which a PowerShell hashtable ignores.
$properties = @(
    @('d', 'G'), @('Ht', 'I'), @('h', 'J'), @('OD', 'K'), @('bf', 'L'), @('B', 'N'), @('b', 'O'), @('ID', 'P'),
    @('tw', 'Q'), @('tf', 'T'), @('tnom', 'W'), @('tdes', 'X'), @('kdes', 'Y'),
    @('x', 'AB'), @('y', 'AC'), @('eo', 'AD'), @('xp', 'AE'), @('yp', 'AF'),
    @('bf/2tf', 'AG'), @('b/t', 'AH'), @('b/tdes', 'AI'), @('h/tw', 'AJ'), @('h/tdes', 'AK'), @('D/t', 'AL'),
    @('Zx', 'AN'), @('Sx', 'AO'), @('rx', 'AP'), @('Zy', 'AR'), @('Sy', 'AS'), @('ry', 'AT'),
    @('Cw', 'AY'), @('C', 'AZ'), @('ro', 'BG'), @('H', 'BH'), @('rts', 'BW'), @('ho', 'BX')
)

$work = Join-Path ([IO.Path]::GetTempPath()) ('aisc-import-' + [guid]::NewGuid())
Copy-Item -LiteralPath $Workbook -Destination "$work.zip"
Expand-Archive -LiteralPath "$work.zip" -DestinationPath $work
try {
    [xml]$shared = Get-Content -Raw -Encoding UTF8 (Join-Path $work 'xl/sharedStrings.xml')
    $strings = @($shared.sst.si | ForEach-Object { $_.InnerText })
    [xml]$workbookXml = Get-Content -Raw -Encoding UTF8 (Join-Path $work 'xl/workbook.xml')
    $sheetNames = @($workbookXml.workbook.sheets.sheet | ForEach-Object { $_.name })
    $index = [array]::IndexOf($sheetNames, 'Database v16.0')
    if ($index -lt 0) { throw "no 'Database v16.0' sheet in $Workbook" }
    [xml]$sheet = Get-Content -Raw -Encoding UTF8 (Join-Path $work "xl/worksheets/sheet$($index + 1).xml")
} finally {
    Remove-Item -LiteralPath $work, "$work.zip" -Recurse -Force
}

function Cell($c) {
    if ($null -eq $c) { return $null }
    if ($c.t -eq 's') { return $strings[[int]$c.v] }
    if ($c.t -eq 'inlineStr') { return $c.is.InnerText }
    return $c.v
}
# The shortest text that reads back as the same double, so 16.100000000000001
# in the sheet is written 16.1.
function Number($text) {
    $value = [double]::Parse($text, [Globalization.CultureInfo]::InvariantCulture)
    return $value.ToString('R', [Globalization.CultureInfo]::InvariantCulture)
}

$rows = @($sheet.worksheet.sheetData.row)
$header = @{}
foreach ($c in $rows[0].c) { $header[($c.r -replace '\d', '')] = Cell $c }
# Refuse a workbook whose columns have moved.
$expected = [ordered]@{ 'A' = 'Type'; 'C' = 'AISC_Manual_Label'; 'F' = 'A'; 'AQ' = 'Iy'; 'AM' = 'Ix'; 'AX' = 'J' }
foreach ($p in $properties) { $expected[$p[1]] = $p[0] }
foreach ($column in $expected.Keys) {
    if ($header[$column] -cne $expected[$column]) { throw "column $column is '$($header[$column])', expected '$($expected[$column])'" }
}

$lines = New-Object System.Collections.Generic.List[string]
$skipped = @{}
foreach ($row in ($rows | Select-Object -Skip 1)) {
    $cells = @{}
    foreach ($c in $row.c) { $cells[($c.r -replace '\d', '')] = Cell $c }
    $kind = $cells['A']
    if (-not $kind) { continue }
    if ($kinds -notcontains $kind) { $skipped[$kind] = 1 + [int]$skipped[$kind]; continue }
    $fields = foreach ($name in $base.Keys) { '"{0}": {1}' -f $name, (Number $cells[$base[$name]]) }
    # The sheet marks a property that does not apply with an en dash.
    $props = foreach ($p in $properties) {
        $text = $cells[$p[1]]
        if ($text -and $text -ne ([string][char]0x2013)) { '"{0}": {1}' -f $p[0], (Number $text) }
    }
    $lines.Add(('    {{ "designation": "{0}", {1}, "shape": {{ "kind": "{2}", "properties": {{ {3} }} }} }}' -f
        $cells['C'], ($fields -join ', '), $kind, ($props -join ', ')))
}

$note = 'AISC Shapes Database v16.0 (August 2023), consistent with the AISC Steel Construction Manual, 16th Edition, ' +
    'published by the American Institute of Steel Construction and converted by scripts/import_aisc_shapes.ps1. ' +
    'AISC makes no warranty for the data; a licensed engineer must verify it for any specific application. ' +
    'Dimensions are in inches and properties in powers of inches. iz is AISC Ix and iy is AISC Iy; ' +
    'properties keep AISC''s names, so Zx, Sx and rx go with iz. ' +
    'Single angles (L) and double angles (2L) are not included.'
$text = "{`n" +
    "  `"name`": `"aisc-shapes`",`n" +
    "  `"version`": `"16.0`",`n" +
    "  `"units`": `"us_customary`",`n" +
    "  `"note`": `"$note`",`n" +
    "  `"sections`": [`n" + ($lines -join ",`n") + "`n  ]`n}`n"
[IO.File]::WriteAllText($Out, $text, (New-Object Text.UTF8Encoding $false))
Write-Output ("{0} sections written to {1}; skipped {2}" -f $lines.Count, $Out,
    (($skipped.GetEnumerator() | Sort-Object Name | ForEach-Object { "$($_.Value) $($_.Name)" }) -join ', '))
