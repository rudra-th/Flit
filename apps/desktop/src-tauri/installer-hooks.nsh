; The identity and the send history live in %APPDATA%\Flit, beside the user's
; profile rather than inside the install directory. Without this the uninstaller
; would leave a private key behind, and a later install would silently adopt it:
; every device that had blocked the old copy would still be talking to the same
; fingerprint. Received files stay in Downloads\Flit; they are the user's, not
; ours to delete.

!macro NSIS_HOOK_PREUNINSTALL
  RMDir /r "$APPDATA\Flit"
!macroend
