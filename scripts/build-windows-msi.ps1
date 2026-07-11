param(
  [string]$NuclExe = "target\release\nucl.exe",
  [string]$RuntimeDir = "crates\nucleotide\runtime",
  [string]$RemoteHelperDir = "target\remote-helpers",
  [string]$GhosttyDll,
  [string]$OutputDir = "target\release\bundle\wxsmsi",
  [string]$OutputName = "nucleotide",
  [string]$ProductVersion,
  [switch]$RequireRemoteHelpers,
  [switch]$GenerateOnly
)

$ErrorActionPreference = "Stop"

function Resolve-RepoPath {
  param([string]$Path)

  if ([System.IO.Path]::IsPathRooted($Path)) {
    return [System.IO.Path]::GetFullPath($Path)
  }

  return [System.IO.Path]::GetFullPath((Join-Path $PWD $Path))
}

function Escape-Xml {
  param([string]$Value)

  return [System.Security.SecurityElement]::Escape($Value)
}

function Get-WixId {
  param(
    [string]$Prefix,
    [string]$Value
  )

  $bytes = [System.Text.Encoding]::UTF8.GetBytes($Value.ToLowerInvariant())
  $sha1 = [System.Security.Cryptography.SHA1]::Create()
  try {
    $hash = -join ($sha1.ComputeHash($bytes) | ForEach-Object { $_.ToString("x2") })
  } finally {
    $sha1.Dispose()
  }

  return "$Prefix$($hash.Substring(0, 32))"
}

function Get-WixGuid {
  param([string]$Value)

  $bytes = [System.Text.Encoding]::UTF8.GetBytes($Value.ToLowerInvariant())
  $sha1 = [System.Security.Cryptography.SHA1]::Create()
  try {
    [byte[]]$guidBytes = $sha1.ComputeHash($bytes)[0..15]
  } finally {
    $sha1.Dispose()
  }

  $guidBytes[6] = ($guidBytes[6] -band 0x0f) -bor 0x50
  $guidBytes[8] = ($guidBytes[8] -band 0x3f) -bor 0x80
  return ([Guid]::new($guidBytes)).ToString("D").ToUpperInvariant()
}

function Get-WorkspaceVersion {
  $cargoToml = Join-Path $PWD "Cargo.toml"
  $content = Get-Content -LiteralPath $cargoToml
  $inWorkspacePackage = $false

  foreach ($line in $content) {
    if ($line -match '^\s*\[workspace\.package\]\s*$') {
      $inWorkspacePackage = $true
      continue
    }

    if ($line -match '^\s*\[') {
      $inWorkspacePackage = $false
    }

    if ($inWorkspacePackage -and $line -match '^\s*version\s*=\s*"([^"]+)"') {
      return $Matches[1]
    }
  }

  throw "Could not find workspace package version in Cargo.toml."
}

function Convert-ToMsiVersion {
  param([string]$Version)

  $baseVersion = ($Version -split '[-+]')[0]
  $parts = $baseVersion.Split(".")
  if ($parts.Count -lt 1 -or $parts.Count -gt 4) {
    throw "Version '$Version' cannot be converted to an MSI version."
  }

  $numericParts = @()
  foreach ($part in $parts) {
    if ($part -notmatch '^\d+$') {
      throw "Version '$Version' contains non-numeric MSI version part '$part'."
    }
    $numericParts += [int]$part
  }

  while ($numericParts.Count -lt 4) {
    $numericParts += 0
  }

  return ($numericParts -join ".")
}

function ConvertTo-RtfEscapedText {
  param([string]$Value)

  $builder = [System.Text.StringBuilder]::new()
  foreach ($char in $Value.ToCharArray()) {
    switch ($char) {
      "\" { [void]$builder.Append("\\") }
      "{" { [void]$builder.Append("\{") }
      "}" { [void]$builder.Append("\}") }
      "`r" { }
      "`n" { [void]$builder.Append("\par").Append([Environment]::NewLine) }
      default {
        $codepoint = [int][char]$char
        if ($codepoint -gt 127) {
          [void]$builder.Append("\u").Append($codepoint).Append("?")
        } else {
          [void]$builder.Append($char)
        }
      }
    }
  }

  return $builder.ToString()
}

function Ensure-LicenseRtf {
  param([string]$Path)

  if (Test-Path -LiteralPath $Path -PathType Leaf) {
    return
  }

  $licensePath = Resolve-RepoPath "LICENSE"
  if (-not (Test-Path -LiteralPath $licensePath -PathType Leaf)) {
    throw "License source not found: $licensePath"
  }

  $licenseText = Get-Content -LiteralPath $licensePath -Raw
  $rtfText = ConvertTo-RtfEscapedText $licenseText
  $rtf = "{\rtf1\ansi\deff0{\fonttbl{\f0 Segoe UI;}}\fs18 $rtfText}"

  New-Item -ItemType Directory -Path ([System.IO.Path]::GetDirectoryName($Path)) -Force | Out-Null
  Set-Content -LiteralPath $Path -Value $rtf -Encoding ASCII
}

function Emit-RuntimeDirectory {
  param(
    [string]$Directory,
    [string]$DirectoryId,
    [string]$Name,
    [string]$RelativePrefix,
    [int]$Level,
    [System.Collections.Generic.List[string]]$ComponentRefs
  )

  $indent = " " * $Level
  $escapedName = Escape-Xml $Name
  $lines = [System.Collections.Generic.List[string]]::new()
  $lines.Add("$indent<Directory Id=`"$DirectoryId`" Name=`"$escapedName`">")

  $cleanupComponentId = Get-WixId "cmp_cleanup_" $RelativePrefix
  $cleanupRegistryName = Get-WixId "cleanup_" $RelativePrefix
  $removeFolderId = Get-WixId "rmf_" $RelativePrefix
  $lines.Add("$indent  <Component Id=`"$cleanupComponentId`" Guid=`"*`">")
  $lines.Add("$indent    <RegistryValue Root=`"HKCU`" Key=`"Software\the nucleotide contributors\Nucleotide\Directories`" Name=`"$cleanupRegistryName`" Type=`"integer`" Value=`"1`" KeyPath=`"yes`"/>")
  $lines.Add("$indent    <RemoveFolder Id=`"$removeFolderId`" Directory=`"$DirectoryId`" On=`"uninstall`"/>")
  $lines.Add("$indent  </Component>")
  $ComponentRefs.Add("      <ComponentRef Id=`"$cleanupComponentId`"/>")

  $files = Get-ChildItem -LiteralPath $Directory -File | Sort-Object Name
  foreach ($file in $files) {
    $relativePath = if ($RelativePrefix) {
      "$RelativePrefix\$($file.Name)"
    } else {
      $file.Name
    }
    $componentId = Get-WixId "cmp_" $relativePath
    $componentGuid = Get-WixGuid "nucleotide:$relativePath"
    $fileId = Get-WixId "fil_" $relativePath
    $source = Escape-Xml $file.FullName

    $lines.Add("$indent  <Component Id=`"$componentId`" Guid=`"$componentGuid`">")
    $lines.Add("$indent    <File Id=`"$fileId`" Source=`"$source`"/>")
    $lines.Add("$indent    <RegistryValue Root=`"HKCU`" Key=`"Software\the nucleotide contributors\Nucleotide\Components`" Name=`"$componentId`" Type=`"integer`" Value=`"1`" KeyPath=`"yes`"/>")
    $lines.Add("$indent  </Component>")
    $ComponentRefs.Add("      <ComponentRef Id=`"$componentId`"/>")
  }

  $directories = Get-ChildItem -LiteralPath $Directory -Directory | Sort-Object Name
  foreach ($child in $directories) {
    $relativePath = if ($RelativePrefix) {
      "$RelativePrefix\$($child.Name)"
    } else {
      $child.Name
    }
    $childId = Get-WixId "dir_" $relativePath
    $childLines = Emit-RuntimeDirectory `
      -Directory $child.FullName `
      -DirectoryId $childId `
      -Name $child.Name `
      -RelativePrefix $relativePath `
      -Level ($Level + 2) `
      -ComponentRefs $ComponentRefs

    foreach ($line in $childLines) {
      $lines.Add($line)
    }
  }

  $lines.Add("$indent</Directory>")
  return $lines
}

function Emit-RemoteHelperComponents {
  param(
    [string]$Directory,
    [int]$Level,
    [System.Collections.Generic.List[string]]$ComponentRefs,
    [switch]$Required
  )

  $indent = " " * $Level
  $lines = [System.Collections.Generic.List[string]]::new()
  $helperNames = @(
    "nucleotide-remote-linux-x86_64",
    "nucleotide-remote-linux-aarch64"
  )

  if (-not (Test-Path -LiteralPath $Directory -PathType Container)) {
    if ($Required) {
      throw "Required Linux remote helper directory not found: $Directory"
    }

    Write-Warning "Linux remote helper directory not found: $Directory"
    return $lines
  }

  foreach ($helperName in $helperNames) {
    $helperPath = Join-Path $Directory $helperName
    if (-not (Test-Path -LiteralPath $helperPath -PathType Leaf)) {
      if ($Required) {
        throw "Required Linux remote helper not found: $helperPath"
      }

      continue
    }

    $componentId = Get-WixId "cmp_remote_helper_" $helperName
    $componentGuid = Get-WixGuid "nucleotide:remote-helper:$helperName"
    $fileId = Get-WixId "fil_remote_helper_" $helperName
    $source = Escape-Xml ([System.IO.Path]::GetFullPath($helperPath))

    $lines.Add("$indent<Component Id=`"$componentId`" Guid=`"$componentGuid`">")
    $lines.Add("$indent  <File Id=`"$fileId`" Name=`"$helperName`" Source=`"$source`"/>")
    $lines.Add("$indent  <RegistryValue Root=`"HKCU`" Key=`"Software\the nucleotide contributors\Nucleotide\Components`" Name=`"$componentId`" Type=`"integer`" Value=`"1`" KeyPath=`"yes`"/>")
    $lines.Add("$indent</Component>")
    $ComponentRefs.Add("      <ComponentRef Id=`"$componentId`"/>")
  }

  return $lines
}

function Resolve-GhosttyDll {
  param(
    [string]$NuclExePath,
    [string]$ExplicitPath
  )

  if ($ExplicitPath) {
    $resolved = Resolve-RepoPath $ExplicitPath
    if (-not (Test-Path -LiteralPath $resolved -PathType Leaf)) {
      throw "Ghostty runtime DLL not found: $resolved"
    }

    return $resolved
  }

  $profileDir = [System.IO.Path]::GetDirectoryName($NuclExePath)
  $directCandidates = @(
    (Join-Path $profileDir "ghostty-vt.dll"),
    (Join-Path $profileDir "deps\ghostty-vt.dll")
  )

  foreach ($candidate in $directCandidates) {
    if (Test-Path -LiteralPath $candidate -PathType Leaf) {
      return [System.IO.Path]::GetFullPath($candidate)
    }
  }

  $buildDir = Join-Path $profileDir "build"
  if (Test-Path -LiteralPath $buildDir -PathType Container) {
    $generatedDll = Get-ChildItem `
      -LiteralPath $buildDir `
      -Filter "ghostty-vt.dll" `
      -File `
      -Recurse `
      -ErrorAction SilentlyContinue |
      Sort-Object LastWriteTimeUtc -Descending |
      Select-Object -First 1

    if ($generatedDll) {
      return $generatedDll.FullName
    }
  }

  throw "ghostty-vt.dll was not found near $NuclExePath. Build Nucleotide with the Ghostty terminal backend first, or pass -GhosttyDll <path>."
}

function Emit-NativeDllComponents {
  param(
    [string[]]$DllPaths,
    [int]$Level,
    [System.Collections.Generic.List[string]]$ComponentRefs
  )

  $indent = " " * $Level
  $lines = [System.Collections.Generic.List[string]]::new()

  foreach ($dllPath in $DllPaths) {
    if (-not (Test-Path -LiteralPath $dllPath -PathType Leaf)) {
      throw "Native runtime DLL not found: $dllPath"
    }

    $file = Get-Item -LiteralPath $dllPath
    $componentId = Get-WixId "cmp_native_dll_" $file.Name
    $componentGuid = Get-WixGuid "nucleotide:native-dll:$($file.Name)"
    $fileId = Get-WixId "fil_native_dll_" $file.Name
    $source = Escape-Xml $file.FullName
    $name = Escape-Xml $file.Name

    $lines.Add("$indent<Component Id=`"$componentId`" Guid=`"$componentGuid`">")
    $lines.Add("$indent  <File Id=`"$fileId`" Name=`"$name`" Source=`"$source`"/>")
    $lines.Add("$indent  <RegistryValue Root=`"HKCU`" Key=`"Software\the nucleotide contributors\Nucleotide\Components`" Name=`"$componentId`" Type=`"integer`" Value=`"1`" KeyPath=`"yes`"/>")
    $lines.Add("$indent</Component>")
    $ComponentRefs.Add("      <ComponentRef Id=`"$componentId`"/>")
  }

  return $lines
}

$nuclExePath = Resolve-RepoPath $NuclExe
$runtimePath = Resolve-RepoPath $RuntimeDir
$remoteHelperPath = Resolve-RepoPath $RemoteHelperDir
$outputPath = Resolve-RepoPath $OutputDir
$templateDir = Resolve-RepoPath "build\windows"
$wxsTemplatePath = Join-Path $templateDir "installer.wxs.in"
$wixprojTemplatePath = Join-Path $templateDir "installer.wixproj.in"
$iconPath = Resolve-RepoPath "crates\nucleotide\assets\nucleotide.ico"
$licenseRtfPath = Resolve-RepoPath "target\release\License.rtf"

if (-not (Test-Path -LiteralPath $nuclExePath -PathType Leaf)) {
  throw "Nucleotide executable not found: $nuclExePath"
}
if (-not (Test-Path -LiteralPath $runtimePath -PathType Container)) {
  throw "Runtime directory not found: $runtimePath"
}
if (-not (Test-Path -LiteralPath $iconPath -PathType Leaf)) {
  throw "Nucleotide icon not found: $iconPath"
}

$ghosttyDllPath = Resolve-GhosttyDll -NuclExePath $nuclExePath -ExplicitPath $GhosttyDll

Write-Host "Bundling Ghostty runtime DLL: $ghosttyDllPath"

Ensure-LicenseRtf $licenseRtfPath

if (-not $ProductVersion) {
  $ProductVersion = Convert-ToMsiVersion (Get-WorkspaceVersion)
}

New-Item -ItemType Directory -Path $outputPath -Force | Out-Null

$componentRefs = [System.Collections.Generic.List[string]]::new()
$remoteHelperComponents = Emit-RemoteHelperComponents `
  -Directory $remoteHelperPath `
  -Level 10 `
  -ComponentRefs $componentRefs `
  -Required:$RequireRemoteHelpers

$nativeDllComponents = Emit-NativeDllComponents `
  -DllPaths @($ghosttyDllPath) `
  -Level 10 `
  -ComponentRefs $componentRefs

$runtimeDirectory = Emit-RuntimeDirectory `
  -Directory $runtimePath `
  -DirectoryId "runtime_Dir" `
  -Name "runtime" `
  -RelativePrefix "runtime" `
  -Level 10 `
  -ComponentRefs $componentRefs

$replacements = @{
  "{{PRODUCT_VERSION}}" = Escape-Xml $ProductVersion
  "{{OUTPUT_NAME}}" = Escape-Xml $OutputName
  "{{NUCL_EXE}}" = Escape-Xml $nuclExePath
  "{{MAIN_EXECUTABLE_GUID}}" = Get-WixGuid "nucleotide:nucl.exe"
  "{{NATIVE_DLL_COMPONENTS}}" = ($nativeDllComponents -join [Environment]::NewLine)
  "{{REMOTE_HELPER_COMPONENTS}}" = ($remoteHelperComponents -join [Environment]::NewLine)
  "{{RUNTIME_DIRECTORY}}" = ($runtimeDirectory -join [Environment]::NewLine)
  "{{COMPONENT_REFS}}" = ($componentRefs -join [Environment]::NewLine)
  "{{ICON_PATH}}" = Escape-Xml $iconPath
  "{{LICENSE_RTF}}" = Escape-Xml $licenseRtfPath
}

$wxs = Get-Content -LiteralPath $wxsTemplatePath -Raw
$wixproj = Get-Content -LiteralPath $wixprojTemplatePath -Raw
foreach ($key in $replacements.Keys) {
  $wxs = $wxs.Replace($key, $replacements[$key])
  $wixproj = $wixproj.Replace($key, $replacements[$key])
}

$wxsPath = Join-Path $outputPath "installer.wxs"
$wixprojPath = Join-Path $outputPath "installer.wixproj"
Set-Content -LiteralPath $wxsPath -Value $wxs -Encoding UTF8
Set-Content -LiteralPath $wixprojPath -Value $wixproj -Encoding UTF8

Write-Host "Generated WiX source at $wxsPath"
Write-Host "Install root: %LOCALAPPDATA%\Spiralpoint\nucleotide"

if ($GenerateOnly) {
  return
}

dotnet build $wixprojPath -c Release
if ($LASTEXITCODE -ne 0) {
  exit $LASTEXITCODE
}

$msi = Join-Path $outputPath "bin\Release\$OutputName.msi"
if (-not (Test-Path -LiteralPath $msi -PathType Leaf)) {
  throw "Windows installer not found: $msi"
}

Write-Host "Windows installer written to $msi"
