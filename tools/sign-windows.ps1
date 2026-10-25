# Sign a Windows binary with the release Authenticode certificate.
#
# The certificate is a self-signed RSA-2048 code-signing pair kept OUTSIDE this
# repository (see E:\Codex\wish\make_cert.py to regenerate it):
#
#   CN=Wish Enhanced, EKU codeSigning, 10 years
#   E:\Codex\wish\wish-enhanced.pfx  (password: wish-enhanced-2026)
#
# A self-signed certificate only turns "unknown publisher" into a named
# publisher: SmartScreen reputation warnings remain until an OV certificate is
# bought and the binaries accumulate reputation. The signature itself is
# cryptographically valid (SHA-256, Windows accepts the digest).
#
# Usage:
#   pwsh -File tools\sign-windows.ps1 dist\wish\wish.exe [more files...]

param(
  [Parameter(Mandatory = $true)][string[]]$Path,
  [string]$Pfx = 'E:\Codex\wish\wish-enhanced.pfx',
  [string]$Password = 'wish-enhanced-2026'
)

$ErrorActionPreference = 'Stop'
$cert = New-Object System.Security.Cryptography.X509Certificates.X509Certificate2(
  @($Pfx), $Password)
if (-not $cert.HasPrivateKey) { throw "the pfx carries no private key" }
Write-Host "signing as $($cert.Subject)"

foreach ($item in $Path) {
  $full = (Resolve-Path $item).Path
  $result = Set-AuthenticodeSignature -FilePath $full -Certificate $cert `
    -HashAlgorithm SHA256 -Force
  $status = (Get-AuthenticodeSignature $full).Status
  # UnknownError here means only that the self-signed root is not in the local
  # trust store; a digest problem would report HashMismatch instead.
  Write-Host ("{0}: {1}" -f $full, $status)
}
