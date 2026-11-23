"use strict";
const TPL = {"vsnd":{"base64":"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","offsets":{"samples":1706,"size":1718,"duration":2414,"curve":1455},"rate":44100,"channels":1},"events":{"base64":"xisAAAwAAQAIAAAAAgAAAFJFRDIcAAAAvw0AAERBVEHQDQAA1h0AAAAAAAAAAAAABTNWS3wWEnTpBphGr/LmPrWQN+cBAAAAAAAAQD0rAAABAAAAAAAAAMgEAADNAAkAHE4AAEcNAAAAAAAAAAAAAAAAAAAAAAAARCsAAMUGAADYIgAAggYAAAQAAAAAAAAAtQYAAAAAAADEBAAAzQAAAAkAAADkAAAA8C1tX0lucHV0RGVwZW5kZW5jaWVzAG1fUmVsYXRpdmVGaWxlbmFtZQBzb3VuZGV2ZW50cy9kYW1hZ2UudnMSABB0LwDwB1NlYXJjaFBhdGgAY2l0YWRlbABtX249AOBDUkMAbV9iT3B0aW9uYRcAEGIXAEFFeGlzOgBhYklzR2FtaAACZAD0AHMvaGl0X2luZGljYXRvcm4A/whfcmVjZWl2ZV9kZWZhdWx0XzAxLm1wMzQAHgC1AA81AB4/d2F2NAAcHzKdACAfMp0AIR8ynQAgHzOdACAfM50AIR8znQAgHzSdACAfNJ0AIR80nQAgHzWdACAfNZ0AIR81nQAgHzadACAfNp0AIR82nQAYH2+qAxwfb6YDHR9vogMcH2+eAxwfb5oDHR9vlgMcH2+SAxwfb44DHR9vigMcH2+GAxwfb4IDHR9vfgMcH296Axwfb3YDHR9vcgMcH29uAxwfb2oDHR9vZgMbb2xldGhhbGkDGgYzAA+XABcGNAAPmgAeHzKaAB8fMpoAIB8ymgAfHzOaAB8fM5oAIB8zmgAfHzSaAB8fNJoAIB80mgAfHzWaAB8fNZoAIB81mgAfHzaaAB8fNpoAIA+cAyAfN5oAHx83mgAgHzeaAB8fOJoAHx84mgAgHziaAB8fOZoAHx85mgAgHzmaAB4vMTCaAB4vMTCaAB8vMTCaAB4/bnBjCAYhAzcAD6IAHgM4AA+mACIfMqYAIx8ypgAkHzKmACMfM6YAIx8zpgAkHzOmACMfNKYAIx80pgAkHzSmACMfNaYAIx81pgAkHzWmACMfNqYAIx82pgAkHzamACMfN6YAIx83pgAkHzemACMfOKYAIx84pgAkHzimACMfOaYAIx85pgAkHzmmACIPdAYgAzcAD6YAIg98BhlfbWVsZWV3BhoFMgAPnAAXBTMAD5cAHR8ylwAeHzKXAB8fMpcAHh8zlwAeHzOXAB8fM5cAHh80lwAeHzSXAB8fNJcAHh81lwAeHzWXAB8fNZcAHh82lwAeHzaXAB8PIgYQj3NlbmRfY3JpbhMTCS4AD48ADwkvAA+LABkfMosAGh8yiwAbHzKLABofM4sAGh8ziwAbHzOLABofNIsAGh80iwAbHzSLABofNYsAGh81iwAbHzWLABofNosAGh82iwAbD0IDFQ9dGhkMMQAPkQAUD1caGQoxAB8ylAAdHzKUAB4PVAMVD0saGQwxAA+UAB0PXQMVD0IaGQwxAA+UAB0PZgMVDzkaGQwxAA+UAB0PbwMVDzAaGQwxAA+UAB0PeAMVD8EWGAswAA+SABQPuxYYCTAAHzKRABwfMpEAHQ9yAxUPrxYYCzAAD5EAHA9vAxUPphYYCzAAD5EAHA9sAxUPnRYYCzAAD5EAHA9pAxUPlBYYCzAAD5EAHA9mAxVfc2hpZWzlBiAOOAAPoQAUDjkAD6kAIx8yqQAkHzKpACUfMqkAJB8zqQAkHzOpACUfM6kAJB80qQAkHzSpACUfNKkAJB81qQAkHzWpACUfNakAJB82qQAkHzapACUP9gMJc3BvaXNvbi8sAA+aAAgLKQAPiwAICyoAD3wABwKoAA98ABALKQAOfAAOKgAPfAAOz3RhbWluYV9kcmFpbnsACQkoAA96AAkJKQAHeQBgdXRpbC9zHieIY2VfMTAwbXNvAA4eAAllAA4fAAFbAGJtX0FkZGkpJw+VJwF8QXJndW1lbhcAoFBhcmFtZXRlck6pJ7JfX19PdmVycmlkZUQAaGF0YV9fXygA8gNUeXBlAEJpbmFyeUJsb2JBcmeuJ5tuZ2VycHJpbnQPABJEeAEA6CdrcGVjaWFsgQCAU3RyaW5nAFPdACAgRSQoiiBTY3JpcHQgVADzBENvbXBpbGVySWRlbnRpZmllcgATAAE7AAE6AAI5AACBAEBVc2VyvQAPWAABdlZlcnNpb26SAA4vAQhNAQHOKCBlZGUoEHOIAPAAaGlsZFJlc291cmNlTGlzbwC1V2Vha1JlZmVyZW4UAALWKEVhYmxljAApSXM9AAAlAHV1YmFzc2V0PQAWcxUAADoBIWluhQAXc0IpIgBEQSlIU2VuZAwAWS5Dcml0EQA5RE9UEACASW52dWxuZXKUAAkZABFMCwgEEwASUkASIS5QtAMMFgARU04DDBYAB2sABRMAA/gBCxcACKYABBQAPE5QQxMAzEJ1bGxldC5NaW5vchwAEE3ZEg8VAAIOSgAEGQAsVDEYAA71AAYWAA9IAAGVU2lkZWNoYWluOwEC3gMgLkTeAw09AU9hdHJvFgAE8AAuUmVzaXN0AAAAAPwAAABfCwAAAAYEAP///wsXBAQAEwEEAFPHAQIVAAwAUwIAAAADIACABQAAAImUmnU4AFMHAAAACCQAGwkkAAwgAB8KIAAMGwsgAEzzqth5JAAfDEQADB8NIAAMGw4gAEwcfrE0JAAfD0QADB8QIAAMGxEgAEyFdz+fJAAfEkQADB8TIAAMGxQgAEz6m6+qJAAfFUQADB8WIAAMGxcgAEzjX3WOJAAfGEQADB8ZIAAMGxogAEwR4iwIJAAfG0QADB8cIAAMGx0gAExi4gy7JAAfHkQADB8fIAAMGyAgAEyuisetJAAfIUQADB8iIAAMGyMgAEyp9IGdJAAfJEQADB8lIAAMGyYgAEwLyrRLJAAfJ0QADB8oIAAMGykgAExATN8pJAAfKkQADB8rIAAMGywgAEyqLis+JAAfLUQADB8uIAAMGy8gAEy7Jb3UJAAfMEQADB8xIAAMGzIgAEwgSkmbJAAfM0QADB80IAAMGzUgAExHhrvnJAAfNkQADB83IAAMGzggAEwFtN1sJAAfOUQADB86IAAMGzsgAEzi4ohJJAAfPEQADB89IAAMGz4gAD31DFKEAx8/RAAMH0AgAAwbQSAATGOd+TYkAB9CRAAMH0MgAAwcRFgCPOQJ+SQAH0VEAAwfRiAADBxHFAU8lAoKJAAfSEQADB9JIAAMG0ogAEyYSmsPJAAfS0QADB9MIAAMHE14BTzC5BQkAB9ORAAMH08gAAwbUCAATDSgbNskAB9RRAAMH1IgAAwbUyAATKidkkMkAB9URAAMH1UgAAwbViAATM9cj6YkAB9XRAAMH1ggAAwbWSAATOysLWIkAB9aRAAMH1sgAAwbXCAATLMIitckAB9dRAAMH14gAAwbXyAATCbqFcckAB9gRAAMH2EgAAwbYiAATGEHgPIkAB9jRAAMH2QgAAwbZSAATDdqIeEkAB9mRAAMH2cgAAwbaCAATGdldLEkAB9pRAAMH2ogAAwbayAATI/cJSskAB9sRAAMH20gAAwbbiAATPf+uzEkAB9vRAAMH3AgAAwbcSAATOBJxgMkAB9yRAAMH3MgAAwcdIAMLTPchAMfdUQADB92IAAMG3cgAEyRPxBHJAAfeEQADB95IAAMG3ogAEwsD0rqJAAfe0QADB98IAAMG30gAEx5iNgEJAAffkQADB9/IAAMG4AgAEweaa26JAAfgUQADB+CIAAMG4MgAExq2rhbJAAfhEQADB+FIAAMHIZABjwDqS8kAB+HRAAMH4ggAAwbiSAATIS6w68kAB+KRAAMH4sgAAwbjCAATAFQLvskAB+NRAAMH44gAAwcj0gNPB+6MiQAH5BEAAwfkSAADBySTAQ8SPB7JAAfk0QADB+UIAAMG5UgAEwwBC23JAAflkQADB+XIAAMG5ggAEzNE07TJAAfmUQADB+aIAAMG5sgAExmFWvaJAAfnEQADB+dIAAMG54gAEwOVJhmJAAfn0QADB+gIAAMG6EgAEwh7iHRJAAfokQADB+jIAAMG6QgAExGeziEJAAfpUQADB+mIAAMG6cgAEyWpTvkJAAfqEQADB+pIAAMG6ogAEztPsnLJAAfq0QADB+sIAAMG60gAExcxFoiJAAfrkQADB+vIAAMG7AgAD3+JuUEEB+xRAAMH7IgAAwbsyAAPXPmR1wSH7REAAwftSAADBy2qBY8K9RFJAAft0QADB+4IAAMG7kgAExUt0R+JAAfukQADB+7IAAMG7wgAEyfwu9tJAAfvUQADB++IAAMG78gAEwzU2W1JAAfwEQADB/BIAAMG8IgAEzHubaZJAAfw0QADB/EIAAMG8UgAEzmhkq/JAAfxkQADB/HIAAMG8ggAExfqPq0JAAfyUQADB/KIAAMG8sgAD3a5swMFx/MRAAMH80gAAwbziAASEQaltgkACLPAAEA8BbQAAAA0QAAANIAAADTAAAA1AAAANUAAADWAAAA1wAAANgAAADZpABDAAAA2xwAE9wYABvdGAATCxwAE95cABPfCAAT4AgAE+EIABDiZBiAAAAA5AAAAOXIAfAFAAAA5wAAAOgAAADpAAAA6gAAAOvwDwBoBfAFAAAA7gAAAO8AAADwAAAA8QAAAPI0GkAAAAD0mBNAAAAA9mANgAAAAPgAAAD5pAxAAAAA+6ga1RgJBgYMDg0OBgYPDQ4GAC8MDRIA/////4UQCK0EIA8PBgDwBRAPBgYMDwgICAgJDwEJGAYA3e7/AAUzVkt8FhJ06QaYRq/y5j61kDfnAQAAAAAAAEBbEwAAAQAAACQAAADaAAAAGgAcAD4dAABeHQAAAAAAAAAAAAAAAAAAAAAAAIAUAACWFAAAvggAAMgIAAAcAAAAAAAAAFABAABDAAAAvgAAABoAAAAWAAAAeAAAAPD//////////////////////////4VEYW1hZ2UuU2VuZABiYXNlAEJhc2UuRGFtYWdlLlNlbmQAdm9sdW1lAHZzbmRfZmlsZXMAc291bmRzL2hpdF9pbmRpY2F0b3JzL2RhbWFnZV9zZW5kX2RlZmF1bHRfMDEudnNuZABzb3VuZHMvaGl0X2luZGljYXRvcnMvZGFtYWdlX3NlbmRfZGVmYXVsdF8wMi52c25kAHNvdW5kcy9oaXRfaW5kaWNhdG9ycy9kYW1hZ2Vfc2VuZF9kZWZhdWx0XzAzLnZzbmQAc291bmRzL2hpdF9pbmRpY2F0b3JzL2RhbWFnZV9zZW5kX2RlZmF1bHRfMDQudnNuZABzb3VuZHMvaGl0X2luZGljYXRvcnMvZGFtYWdlX3NlbmRfZGVmYXVsdF8wNS52c25kAHNvdW5kcy9oaXRfaW5kaWNhdG9ycy9kYW1hZ2Vfc2VuZF9kZWZhdWx0XzA2LnZzbmQAaW5zdGFuY2VfY291bnRfb2Zmc2V0X2Ftb3VudF9kYgB2c25kX2R1cmF0aW9uAERhbWFnZS5TZW5kLkNyaXQAdm9sdW1lX2ZhZGVfb3V0AGxpbWl0ZXJfb24AbGltaXRlcl9tYXgAbGltaXRlcl9zdG9wX29sZGVzdABzb3VuZHMvaGl0X2luZGljYXRvcnMvZGFtYWdlX3NlbmRfY3JpdF8wMS52c25kAHNvdW5kcy9oaXRfaW5kaWNhdG9ycy9kYW1hZ2Vfc2VuZF9jcml0XzAyLnZzbmQAc291bmRzL2hpdF9pbmRpY2F0b3JzL2RhbWFnZV9zZW5kX2NyaXRfMDMudnNuZABzb3VuZHMvaGl0X2luZGljYXRvcnMvZGFtYWdlX3NlbmRfY3JpdF8wNC52c25kAHNvdW5kcy9oaXRfaW5kaWNhdG9ycy9kYW1hZ2Vfc2VuZF9jcml0XzA1LnZzbmQAc291bmRzL2hpdF9pbmRpY2F0b3JzL2RhbWFnZV9zZW5kX2NyaXRfMDYudnNuZABEYW1hZ2UuU2VuZC5ET1QAcGl0Y2gAdm9sdW1lX2ZhbGxvZmZfbWluAHZvbHVtZV9mYWxsb2ZmX21heAByZWNlbmN5X2JpYXNfbWF4X3NjYWxlAERhbWFnZS5TZW5kLkludnVsbmVyYWJsZQBibG9ja19kdXJhdGlvbgBzb3VuZHMvaGl0X2luZGljYXRvcnMvc2hpZWxkL2RlZmF1bHQudnNuZABEYW1hZ2UuU2VuZC5MZXRoYWwAc291bmRzL2hpdF9pbmRpY2F0b3JzL2RhbWFnZV9zZW5kX2xldGhhbF8wMS52c25kAHNvdW5kcy9oaXRfaW5kaWNhdG9ycy9kYW1hZ2Vfc2VuZF9sZXRoYWxfMDIudnNuZABzb3VuZHMvaGl0X2luZGljYXRvcnMvZGFtYWdlX3NlbmRfbGV0aGFsXzAzLnZzbmQAc291bmRzL2hpdF9pbmRpY2F0b3JzL2RhbWFnZV9zZW5kX2xldGhhbF8wNC52c25kAHNvdW5kcy9oaXRfaW5kaWNhdG9ycy9kYW1hZ2Vfc2VuZF9sZXRoYWxfMDUudnNuZABzb3VuZHMvaGl0X2luZGljYXRvcnMvZGFtYWdlX3NlbmRfbGV0aGFsXzA2LnZzbmQARGFtYWdlLlJlY2VpdmUuUG9pc29uAEJhc2UuRGFtYWdlLlJlY2VpdmUAc291bmRzL2hpdF9pbmRpY2F0b3JzL3BvaXNvbi9kZWZhdWx0LnZzbmQARGFtYWdlLlJlY2VpdmUuU2hpZWxkAGhlYWx0aF9mcmFjdGlvbl9jdXJ2ZV9lbmFibGVkAGRhbWFnZV9lZmZlY3RpdmVuZXNzX2VuYWJsZWQAY2hpcF9kYW1hZ2Vfb2Zmc2V0X2RiAGluc3RhbmNlX2F0dGVudWF0b3JfZW5hYmxlZAB0cmlnZ2Vyc19kYW1hZ2Vfc2lkZWNoYWluAGxpbWl0ZXJfZ2xvYmFsX2NhdGVnb3J5X2Rpc3RhbmNlX29uAHNvdW5kcy9oaXRfaW5kaWNhdG9ycy9kYW1hZ2Vfc2VuZF9zaGllbGRfZGVmYXVsdF8wMS52c25kAHNvdW5kcy9oaXRfaW5kaWNhdG9ycy9kYW1hZ2Vfc2VuZF9zaGllbGRfZGVmYXVsdF8wMi52c25kAHNvdW5kcy9oaXRfaW5kaWNhdG9ycy9kYW1hZ2Vfc2VuZF9zaGllbGRfZGVmYXVsdF8wMy52c25kAHNvdW5kcy9oaXRfaW5kaWNhdG9ycy9kYW1hZ2Vfc2VuZF9zaGllbGRfZGVmYXVsdF8wNC52c25kAHNvdW5kcy9oaXRfaW5kaWNhdG9ycy9kYW1hZ2Vfc2VuZF9zaGllbGRfZGVmYXVsdF8wNS52c25kAHNvdW5kcy9oaXRfaW5kaWNhdG9ycy9kYW1hZ2Vfc2VuZF9zaGllbGRfZGVmYXVsdF8wNi52c25kAERhbWFnZS5SZWNlaXZlLkRPVABibG9ja19hdHRhY2tlcgBzb3VuZHMvaGl0X2luZGljYXRvcnMvZGFtYWdlX3JlY2VpdmVfZG90XzAxLnZzbmQAc291bmRzL2hpdF9pbmRpY2F0b3JzL2RhbWFnZV9yZWNlaXZlX2RvdF8wMi52c25kAHNvdW5kcy9oaXRfaW5kaWNhdG9ycy9kYW1hZ2VfcmVjZWl2ZV9kb3RfMDMudnNuZABzb3VuZHMvaGl0X2luZGljYXRvcnMvZGFtYWdlX3JlY2VpdmVfZG90XzA0LnZzbmQAc291bmRzL2hpdF9pbmRpY2F0b3JzL2RhbWFnZV9yZWNlaXZlX2RvdF8wNS52c25kAHNvdW5kcy9oaXRfaW5kaWNhdG9ycy9kYW1hZ2VfcmVjZWl2ZV9kb3RfMDYudnNuZABEYW1hZ2UuUmVjZWl2ZS5EZWZhdWx0AHNvdW5kcy9oaXRfaW5kaWNhdG9ycy9kYW1hZ2VfcmVjZWl2ZV9kZWZhdWx0XzAxLnZzbmQAc291bmRzL2hpdF9pbmRpY2F0b3JzL2RhbWFnZV9yZWNlaXZlX2RlZmF1bHRfMDIudnNuZABzb3VuZHMvaGl0X2luZGljYXRvcnMvZGFtYWdlX3JlY2VpdmVfZGVmYXVsdF8wMy52c25kAHNvdW5kcy9oaXRfaW5kaWNhdG9ycy9kYW1hZ2VfcmVjZWl2ZV9kZWZhdWx0XzA0LnZzbmQAc291bmRzL2hpdF9pbmRpY2F0b3JzL2RhbWFnZV9yZWNlaXZlX2RlZmF1bHRfMDUudnNuZABzb3VuZHMvaGl0X2luZGljYXRvcnMvZGFtYWdlX3JlY2VpdmVfZGVmYXVsdF8wNi52c25kAERhbWFnZS5SZWNlaXZlLkNyaXQAaW5zdGFuY2VfYXR0ZW51YXRvcl9ncm91cF9uYW1lX3ByZWZpeABkYW1hZ2UtY3JpdC0ARGFtYWdlLlJlY2VpdmUuTlBDAERhbWFnZS5SZWNlaXZlLkJ1bGxldC5NaW5vcgBEYW1hZ2UuUmVjZWl2ZS5NZWxlZQBzb3VuZHMvaGl0X2luZGljYXRvcnMvZGFtYWdlX3JlY2VpdmVfbWVsZWVfMDEudnNuZABzb3VuZHMvaGl0X2luZGljYXRvcnMvZGFtYWdlX3JlY2VpdmVfbWVsZWVfMDIudnNuZABzb3VuZHMvaGl0X2luZGljYXRvcnMvZGFtYWdlX3JlY2VpdmVfbWVsZWVfMDMudnNuZABzb3VuZHMvaGl0X2luZGljYXRvcnMvZGFtYWdlX3JlY2VpdmVfbWVsZWVfMDQudnNuZABzb3VuZHMvaGl0X2luZGljYXRvcnMvZGFtYWdlX3JlY2VpdmVfbWVsZWVfMDUudnNuZABzb3VuZHMvaGl0X2luZGljYXRvcnMvZGFtYWdlX3JlY2VpdmVfbWVsZWVfMDYudnNuZABEYW1hZ2UuUmVjZWl2ZS5NZWxlZS5OUEMARGFtYWdlLlJlY2VpdmUuTWVsZWUuVDEARGFtYWdlLlJlY2VpdmUuTGV0aGFsAHR5cGUAY2l0YWRlbF9wZXJzcGVjdGl2ZV9kZWZhdWx0AGV2ZW50X25hbWUAVm9sdW1lLkZhbGxvZmYuSGVybwBCYXNlLkRhbWFnZS5SZWNlaXZlLkxldGhhbABtaXhfZ3JvdXBfcHJlZml4AERlYXRoLUhlcm8AcGxheV9mcm9tX2xpc3RlbmVyX3BsYXllcgBzb3VuZHMvaGl0X2luZGljYXRvcnMvZGFtYWdlX3JlY2VpdmVfbGV0aGFsXzAxLnZzbmQAc291bmRzL2hpdF9pbmRpY2F0b3JzL2RhbWFnZV9yZWNlaXZlX2xldGhhbF8wMi52c25kAHNvdW5kcy9oaXRfaW5kaWNhdG9ycy9kYW1hZ2VfcmVjZWl2ZV9sZXRoYWxfMDMudnNuZABzb3VuZHMvaGl0X2luZGljYXRvcnMvZGFtYWdlX3JlY2VpdmVfbGV0aGFsXzA0LnZzbmQAc291bmRzL2hpdF9pbmRpY2F0b3JzL2RhbWFnZV9yZWNlaXZlX2xldGhhbF8wNS52c25kAHNvdW5kcy9oaXRfaW5kaWNhdG9ycy9kYW1hZ2VfcmVjZWl2ZV9sZXRoYWxfMDYudnNuZABzb3VuZHMvaGl0X2luZGljYXRvcnMvZGFtYWdlX3JlY2VpdmVfbGV0aGFsXzA3LnZzbmQAc291bmRzL2hpdF9pbmRpY2F0b3JzL2RhbWFnZV9yZWNlaXZlX2xldGhhbF8wOC52c25kAHNvdW5kcy9oaXRfaW5kaWNhdG9ycy9kYW1hZ2VfcmVjZWl2ZV9sZXRoYWxfMDkudnNuZABzb3VuZHMvaGl0X2luZGljYXRvcnMvZGFtYWdlX3JlY2VpdmVfbGV0aGFsXzEwLnZzbmQARGFtYWdlLlJlY2VpdmUuTGV0aGFsLk5QQwBWb2x1bWUuRmFsbG9mZi5OUEMuVHJvb3BlcgBEZWF0aC1OUEMAcG9zaXRpb25fZm9yY2VfZnJvbV9pbnB1dABzb3VuZHMvaGl0X2luZGljYXRvcnMvZGFtYWdlX3JlY2VpdmVfbGV0aGFsX25wY18wMS52c25kAHNvdW5kcy9oaXRfaW5kaWNhdG9ycy9kYW1hZ2VfcmVjZWl2ZV9sZXRoYWxfbnBjXzAyLnZzbmQAc291bmRzL2hpdF9pbmRpY2F0b3JzL2RhbWFnZV9yZWNlaXZlX2xldGhhbF9ucGNfMDMudnNuZABzb3VuZHMvaGl0X2luZGljYXRvcnMvZGFtYWdlX3JlY2VpdmVfbGV0aGFsX25wY18wNC52c25kAHNvdW5kcy9oaXRfaW5kaWNhdG9ycy9kYW1hZ2VfcmVjZWl2ZV9sZXRoYWxfbnBjXzA1LnZzbmQAc291bmRzL2hpdF9pbmRpY2F0b3JzL2RhbWFnZV9yZWNlaXZlX2xldGhhbF9ucGNfMDYudnNuZABzb3VuZHMvaGl0X2luZGljYXRvcnMvZGFtYWdlX3JlY2VpdmVfbGV0aGFsX25wY18wNy52c25kAHNvdW5kcy9oaXRfaW5kaWNhdG9ycy9kYW1hZ2VfcmVjZWl2ZV9sZXRoYWxfbnBjXzA4LnZzbmQAc291bmRzL2hpdF9pbmRpY2F0b3JzL2RhbWFnZV9yZWNlaXZlX2xldGhhbF9ucGNfMDkudnNuZABzb3VuZHMvaGl0X2luZGljYXRvcnMvZGFtYWdlX3JlY2VpdmVfbGV0aGFsX25wY18xMC52c25kAERhbWFnZS5SZWNlaXZlLlNpZGVjaGFpbgBjaXRhZGVsX2RlZmF1bHRfMmQAbWl4ZXJfbWl4Z3JvdXAARE1HLVJlY2VpdmUtVHJpZ2dlcgBzb3VuZHMvdXRpbC9TaWxlbmNlXzEwMG1zLnZzbmQARGFtYWdlLlN0YW1pbmEuRHJhaW4AQmFzZS5BYmlsaXR5AHNvdW5kcy9oaXRfaW5kaWNhdG9ycy9zdGFtaW5hX2RyYWluLnZzbmQARGFtYWdlLlJlY2VpdmUuUGF0cm9uAG1peGVyX21peGdyb3VwX2F0dGFja2VyAERtZy1QYXRyb24tQXR0YWNrZXIAbWl4ZXJfbWl4Z3JvdXBfdmljdGltAERtZy1QYXRyb24tVmljdGltAG1peGVyX21peGdyb3VwX290aGVyAERtZy1QYXRyb24tT3RoZXIAdm9sdW1lX3ZpY3RpbV9ucGNfb2Zmc2V0X2RiAGxpbWl0ZXJfZ2xvYmFsX2NhdGVnb3J5X25hbWUAZGFtYWdlLXBhdHJvbgBibG9ja192aWN0aW0AYmxvY2tfb3RoZXIARGFtYWdlLlJlY2VpdmUuUGF0cm9uLlJlc2lzdABwaXRjaF9yYW5kX21pbgBwaXRjaF9yYW5kX21heABzcHJlYWRfbWluX3ZhbHVlAHNwcmVhZF9tYXhfdmFsdWUAb2NjbHVzaW9uX3NjYWxlX25vbl92aWN0aW0Adm9sdW1lX2ZhbGxvZmZfbWF4X2F0dGFja2VyAG1heF9kaXN0YW5jZV9yZWNpcGllbnRfZmlsdGVyAACRAAAAv2GiQQpe8j8AAAAAAAAAAAAAAAAAAAAAAI49ey6zAMAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAIEBv9gfKbZsswACOPXsuswDAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAACEAAAAAAAAAAAAAAAAAAAAAAv0nToGh+D8AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAGEAAAAAAAAAiwL9J06Bofg/AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAv2GiQQpe8j8AAAAAAAAAAAAAAAAAAAAAAI49ey6zAMAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAIEBv9gfKbZsswACOPXsuswDAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA8P//////////txUAAAAFAAAACQAAAAcAAAAFAAAABAAAAAUAAAAOAAAA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","offsets":{"body.volume":10624,"body.pitch":10632,"body.duration":10640,"head.send.volume":10464,"head.send.pitch":10488,"head.send.duration":10480,"head.recv.volume":10744,"head.recv.pitch":10752,"head.recv.duration":10768}}};
const RATE = 44100;
const BASE = { body: 6.3, headSend: 6.3, headRecv: 4.5 };   // pak13 levels at 0 dB

// ---------- bytes helpers ----------
const b64 = s => Uint8Array.from(atob(s), c => c.charCodeAt(0));
const CRC_TABLE = (() => { const t = new Uint32Array(256); for (let i = 0; i < 256; i++) { let c = i; for (let k = 0; k < 8; k++) c = c & 1 ? 0xEDB88320 ^ (c >>> 1) : c >>> 1; t[i] = c >>> 0; } return t; })();
function crc32(u8) { let c = 0xFFFFFFFF; for (let i = 0; i < u8.length; i++) c = CRC_TABLE[(c ^ u8[i]) & 255] ^ (c >>> 8); return (c ^ 0xFFFFFFFF) >>> 0; }
function concat(parts) { let n = 0; for (const p of parts) n += p.length; const out = new Uint8Array(n); let o = 0; for (const p of parts) { out.set(p, o); o += p.length; } return out; }

// ---------- decoding: WAV (PCM, float, MS-ADPCM) by hand, everything else via the browser ----------
function parseWav(buf) {
  const dv = new DataView(buf); const u8 = new Uint8Array(buf);
  const tag = o => String.fromCharCode(u8[o], u8[o + 1], u8[o + 2], u8[o + 3]);
  if (buf.byteLength < 12 || tag(0) !== "RIFF" || tag(8) !== "WAVE") return null;
  let fmt = null, data = null;
  for (let o = 12; o + 8 <= buf.byteLength;) {
    const id = tag(o), len = dv.getUint32(o + 4, true), body = o + 8;
    if (id === "fmt ") fmt = { tag: dv.getUint16(body, true), ch: dv.getUint16(body + 2, true), rate: dv.getUint32(body + 4, true), align: dv.getUint16(body + 12, true), bits: dv.getUint16(body + 14, true), body, len };
    else if (id === "data") data = { off: body, len: Math.min(len, buf.byteLength - body) };
    o = body + len + (len & 1);
  }
  if (!fmt || !data || !fmt.ch) return null;
  let t = fmt.tag;
  if (t === 0xFFFE && fmt.len >= 40) t = dv.getUint16(fmt.body + 24, true);   // WAVE_FORMAT_EXTENSIBLE
  const ch = fmt.ch, out = [];
  if (t === 1 || t === 3) {
    const bps = fmt.bits / 8, frames = Math.floor(data.len / (bps * ch));
    for (let c = 0; c < ch; c++) out.push(new Float32Array(frames));
    for (let i = 0; i < frames; i++) for (let c = 0; c < ch; c++) {
      const p = data.off + (i * ch + c) * bps; let v;
      if (t === 3) v = bps === 8 ? dv.getFloat64(p, true) : dv.getFloat32(p, true);
      else if (bps === 1) v = (u8[p] - 128) / 128;
      else if (bps === 2) v = dv.getInt16(p, true) / 32768;
      else if (bps === 3) v = ((u8[p] | (u8[p + 1] << 8) | (u8[p + 2] << 16)) << 8 >> 8) / 8388608;
      else v = dv.getInt32(p, true) / 2147483648;
      out[c][i] = v;
    }
    return { rate: fmt.rate, channels: out };
  }
  if (t === 2) return decodeMsAdpcm(dv, fmt, data);
  return null;
}
function decodeMsAdpcm(dv, fmt, data) {
  const ADAPT = [230, 230, 230, 230, 307, 409, 512, 614, 768, 614, 512, 409, 307, 230, 230, 230];
  let coefs = [[256, 0], [512, -256], [0, 0], [192, 64], [240, 0], [460, -208], [392, -232]];
  let spb = 0;
  if (fmt.len >= 22) {
    spb = dv.getUint16(fmt.body + 18, true);
    const n = dv.getUint16(fmt.body + 20, true);
    if (n > 0 && fmt.len >= 22 + n * 4) { coefs = []; for (let i = 0; i < n; i++) coefs.push([dv.getInt16(fmt.body + 22 + i * 4, true), dv.getInt16(fmt.body + 24 + i * 4, true)]); }
  }
  const ch = fmt.ch, align = fmt.align;
  if (!spb) spb = Math.floor((align - 7 * ch) * 8 / (4 * ch)) + 2;
  const blocks = Math.floor(data.len / align), out = [];
  for (let c = 0; c < ch; c++) out.push(new Float32Array(blocks * spb));
  const clamp = v => v > 32767 ? 32767 : v < -32768 ? -32768 : v;
  for (let b = 0; b < blocks; b++) {
    let p = data.off + b * align; const st = [];
    for (let c = 0; c < ch; c++) st.push({ co: coefs[Math.min(dv.getUint8(p + c), coefs.length - 1)] });
    p += ch;
    for (let c = 0; c < ch; c++) st[c].delta = dv.getInt16(p + c * 2, true); p += 2 * ch;
    for (let c = 0; c < ch; c++) st[c].s1 = dv.getInt16(p + c * 2, true); p += 2 * ch;
    for (let c = 0; c < ch; c++) st[c].s2 = dv.getInt16(p + c * 2, true); p += 2 * ch;
    const base = b * spb;
    for (let c = 0; c < ch; c++) { out[c][base] = st[c].s2 / 32768; out[c][base + 1] = st[c].s1 / 32768; }
    let idx = 2, c = 0;
    const end = data.off + (b + 1) * align;
    const step = nib => {
      const s = st[c]; const signed = nib >= 8 ? nib - 16 : nib;
      const pred = (s.s1 * s.co[0] + s.s2 * s.co[1]) >> 8;
      const v = clamp(pred + signed * s.delta);
      s.s2 = s.s1; s.s1 = v;
      s.delta = Math.max(16, (ADAPT[nib] * s.delta) >> 8);
      if (base + idx < out[c].length) out[c][base + idx] = v / 32768;
      if (++c === ch) { c = 0; idx++; }
    };
    for (; p < end && idx < spb; p++) { const byte = dv.getUint8(p); step(byte >> 4); if (idx < spb) step(byte & 15); }
  }
  return { rate: fmt.rate, channels: out };
}
async function toMono44k(decoded) {
  const { rate, channels } = decoded, len = channels[0].length;
  if (rate === RATE && channels.length === 1) return channels[0];
  if (rate === RATE) { const m = new Float32Array(len); for (const c of channels) for (let i = 0; i < len; i++) m[i] += c[i] / channels.length; return m; }
  const outLen = Math.max(1, Math.ceil(len * RATE / rate));
  const ctx = new OfflineAudioContext(1, outLen, RATE);
  const ab = ctx.createBuffer(channels.length, len, rate);
  channels.forEach((c, i) => ab.copyToChannel(c, i));
  const src = ctx.createBufferSource(); src.buffer = ab; src.connect(ctx.destination); src.start();
  return (await ctx.startRendering()).getChannelData(0).slice();
}
async function decodeAny(buf) {
  let d = null;
  try { d = parseWav(buf); } catch (e) { d = null; }
  if (!d) {
    const ctx = new OfflineAudioContext(1, 1, RATE);
    const ab = await ctx.decodeAudioData(buf.slice(0));
    d = { rate: ab.sampleRate, channels: Array.from({ length: ab.numberOfChannels }, (_, i) => ab.getChannelData(i).slice()) };
  }
  if (!d.channels[0].length) throw new Error("The file has no audio.");
  return toMono44k(d);
}

// ---------- processing ----------
// first sample louder than -50 dBFS, minus 1 ms (0 if the file is silent)
function soundStart(raw) {
  let i = 0; while (i < raw.length && Math.abs(raw[i]) < 0.00316) i++;
  return i >= raw.length ? 0 : Math.max(0, i - 44);
}
// opts: {startMs, endMs} (endMs null = to the end). Short fades where the cut lands mid-sound avoid clicks.
function processSamples(raw, opts) {
  const start = Math.min(raw.length - 1, Math.max(0, Math.round((opts.startMs || 0) / 1000 * RATE)));
  const endAt = opts.endMs == null ? raw.length : Math.round(opts.endMs / 1000 * RATE);
  const end = Math.min(raw.length, Math.max(start + 1, endAt));
  const out = raw.slice(start, end);
  if (start > 0) { const f = Math.min(out.length, Math.round(0.001 * RATE)); for (let k = 0; k < f; k++) out[k] *= k / f; }
  if (end < raw.length) { const f = Math.min(out.length, Math.round(0.005 * RATE)); for (let k = 0; k < f; k++) out[out.length - 1 - k] *= k / f; }
  return out;
}

// ---------- MP3 encode + frame scan ----------
function encodeMp3(samples) {
  const enc = new lamejs.Mp3Encoder(1, RATE, 128);
  const pcm = new Int16Array(samples.length);
  for (let i = 0; i < samples.length; i++) { const v = Math.max(-1, Math.min(1, samples[i])); pcm[i] = v < 0 ? v * 32768 : v * 32767; }
  const parts = [];
  for (let i = 0; i < pcm.length; i += 1152) { const o = enc.encodeBuffer(pcm.subarray(i, i + 1152)); if (o.length) parts.push(new Uint8Array(o)); }
  const f = enc.flush(); if (f.length) parts.push(new Uint8Array(f));
  return concat(parts);
}
const KBPS = [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320];
function scanMp3(u8) {
  let i = 0, frames = 0, start = -1;
  while (i + 4 <= u8.length) {
    if (u8[i] === 0xFF && (u8[i + 1] & 0xE0) === 0xE0) {
      const ver = (u8[i + 1] >> 3) & 3, layer = (u8[i + 1] >> 1) & 3, br = u8[i + 2] >> 4, sr = (u8[i + 2] >> 2) & 3, pad = (u8[i + 2] >> 1) & 1;
      if (ver === 3 && layer === 1 && br > 0 && br < 15 && sr === 0) {
        const len = Math.floor(144000 * KBPS[br] / RATE) + pad;
        if (i + len > u8.length) break;
        if (start < 0) start = i;
        frames++; i += len; continue;
      }
    }
    if (start >= 0) break;
    i++;
  }
  if (start < 0 || !frames) throw new Error("The MP3 encoder produced no audio frames.");
  return { audio: u8.slice(start, i), frames, samples: frames * 1152 };
}

// MP3 encoding puts ~1105 samples of silence before the audio (576 encoder delay + 529 decoder delay).
// Like the stock game files, prepend a LAME "Info" frame recording that delay and the end padding, so
// gapless decoders skip it, and store the true input length as the sample count.
const ENC_DELAY = 576;
function crc16(u8, n) { let c = 0; for (let i = 0; i < n; i++) { c ^= u8[i]; for (let k = 0; k < 8; k++) c = c & 1 ? (c >>> 1) ^ 0xA001 : c >>> 1; } return c; }
function withInfoTag(mp3, inputSamples) {
  const len = Math.floor(144000 * 128 / RATE), f = new Uint8Array(len), dv = new DataView(f.buffer);
  f.set(mp3.audio.subarray(0, 4)); f[2] = (9 << 4) | (f[2] & 0x0C);        // 128 kbps, no padding bit
  const mono = (f[3] >> 6) === 3; let p = 4 + (mono ? 17 : 32);
  const total = len + mp3.audio.length, padding = Math.max(0, mp3.frames * 1152 - ENC_DELAY - inputSamples);
  f.set([73, 110, 102, 111], p);                                         // "Info" (CBR)
  dv.setUint32(p + 4, 15); dv.setUint32(p + 8, mp3.frames); dv.setUint32(p + 12, total);
  for (let i = 0; i < 100; i++) f[p + 16 + i] = Math.floor(i * 256 / 100);   // TOC
  dv.setUint32(p + 116, 0);                                              // quality
  const q = p + 120;
  f.set([76, 65, 77, 69, 51, 46, 49, 48, 48], q);                        // "LAME3.100"
  f[q + 9] = 0x01; f[q + 20] = 128;                                      // CBR, bitrate
  f[q + 21] = ENC_DELAY >> 4; f[q + 22] = ((ENC_DELAY & 15) << 4) | (padding >> 8); f[q + 23] = padding & 255;
  dv.setUint32(q + 28, total);                                           // music length
  dv.setUint16(q + 34, crc16(f, q + 34));                                // tag CRC
  return { audio: concat([f, mp3.audio]), frames: mp3.frames + 1, samples: inputSamples };
}

// ---------- game files ----------
function buildVsnd(mp3) {
  const t = b64(TPL.vsnd.base64), o = TPL.vsnd.offsets;
  const out = new Uint8Array(t.length + mp3.audio.length);
  out.set(t); out.set(mp3.audio, t.length);
  const dv = new DataView(out.buffer);
  const dur = mp3.samples / RATE;
  // bytes 0-3 stay the template's resource size: it excludes the MP3 appended after the blocks
  dv.setUint32(o.samples, mp3.samples, true);
  dv.setUint32(o.size, mp3.audio.length, true);
  dv.setFloat64(o.duration, dur, true);
  dv.setFloat32(o.curve, dur, true);
  return out;
}
function buildEvents(body, head) {
  const out = b64(TPL.events.base64), o = TPL.events.offsets, dv = new DataView(out.buffer);
  const put = (k, v) => dv.setFloat64(o[k], v, true);
  put("body.volume", BASE.body + body.db); put("body.pitch", body.pitch); put("body.duration", body.duration / body.pitch);
  put("head.send.volume", BASE.headSend + head.db); put("head.send.pitch", head.pitch); put("head.send.duration", head.duration / head.pitch);
  put("head.recv.volume", BASE.headRecv + head.db); put("head.recv.pitch", head.pitch); put("head.recv.duration", head.duration / head.pitch);
  return out;
}
function buildVpk(files) {   // files: [{path, data}] -> VPK v1, everything in the _dir file
  const enc = new TextEncoder(), groups = new Map();
  for (const f of files) {
    const slash = f.path.lastIndexOf("/"), dot = f.path.lastIndexOf(".");
    const ext = f.path.slice(dot + 1), dir = f.path.slice(0, slash), name = f.path.slice(slash + 1, dot);
    if (!groups.has(ext)) groups.set(ext, new Map());
    const g = groups.get(ext); if (!g.has(dir)) g.set(dir, []);
    g.get(dir).push({ name, data: f.data });
  }
  const tree = [], blobs = []; let offset = 0;
  const str = s => { tree.push(enc.encode(s), new Uint8Array([0])); };
  for (const [ext, dirs] of [...groups].sort()) {
    str(ext);
    for (const [dir, list] of [...dirs].sort()) {
      str(dir);
      for (const f of list.sort((a, b) => a.name < b.name ? -1 : 1)) {
        str(f.name);
        const e = new DataView(new ArrayBuffer(18));
        e.setUint32(0, crc32(f.data), true); e.setUint16(4, 0, true); e.setUint16(6, 0x7fff, true);
        e.setUint32(8, offset, true); e.setUint32(12, f.data.length, true); e.setUint16(16, 0xffff, true);
        tree.push(new Uint8Array(e.buffer)); blobs.push(f.data); offset += f.data.length;
      }
      tree.push(new Uint8Array([0]));
    }
    tree.push(new Uint8Array([0]));
  }
  tree.push(new Uint8Array([0]));
  const t = concat(tree), h = new DataView(new ArrayBuffer(12));
  h.setUint32(0, 0x55AA1234, true); h.setUint32(4, 1, true); h.setUint32(8, t.length, true);
  return concat([new Uint8Array(h.buffer), t, ...blobs]);
}
function buildZip(name, data) {   // one stored file
  const n = new TextEncoder().encode(name), crc = crc32(data);
  const d = new Date(), time = (d.getHours() << 11) | (d.getMinutes() << 5) | (d.getSeconds() >> 1), date = ((d.getFullYear() - 1980) << 9) | ((d.getMonth() + 1) << 5) | d.getDate();
  const lh = new DataView(new ArrayBuffer(30));
  [[0, 0x04034b50, 4], [4, 20, 2], [6, 0, 2], [8, 0, 2], [10, time, 2], [12, date, 2], [14, crc, 4], [18, data.length, 4], [22, data.length, 4], [26, n.length, 2], [28, 0, 2]]
    .forEach(([o, v, s]) => s === 4 ? lh.setUint32(o, v, true) : lh.setUint16(o, v, true));
  const cd = new DataView(new ArrayBuffer(46));
  [[0, 0x02014b50, 4], [4, 20, 2], [6, 20, 2], [8, 0, 2], [10, 0, 2], [12, time, 2], [14, date, 2], [16, crc, 4], [20, data.length, 4], [24, data.length, 4], [28, n.length, 2], [30, 0, 2], [32, 0, 2], [34, 0, 2], [36, 0, 2], [38, 0, 4], [42, 0, 4]]
    .forEach(([o, v, s]) => s === 4 ? cd.setUint32(o, v, true) : cd.setUint16(o, v, true));
  const cdOff = 30 + n.length + data.length, cdLen = 46 + n.length;
  const end = new DataView(new ArrayBuffer(22));
  [[0, 0x06054b50, 4], [4, 0, 2], [6, 0, 2], [8, 1, 2], [10, 1, 2], [12, cdLen, 4], [16, cdOff, 4], [20, 0, 2]]
    .forEach(([o, v, s]) => s === 4 ? end.setUint32(o, v, true) : end.setUint16(o, v, true));
  return concat([new Uint8Array(lh.buffer), n, data, new Uint8Array(cd.buffer), n, new Uint8Array(end.buffer)]);
}
// samples per slot + settings -> pak bytes
function buildPak(bodySamples, bodySet, headSamples, headSet) {
  const make = s => { const mp3 = withInfoTag(scanMp3(encodeMp3(s)), s.length); return { vsnd: buildVsnd(mp3), duration: mp3.samples / RATE }; };
  const b = make(bodySamples), h = make(headSamples);
  const files = [];
  for (let i = 1; i <= 6; i++) {
    files.push({ path: `sounds/hit_indicators/damage_send_default_0${i}.vsnd_c`, data: b.vsnd });
    files.push({ path: `sounds/hit_indicators/damage_send_crit_0${i}.vsnd_c`, data: h.vsnd });
  }
  files.push({ path: "soundevents/damage.vsndevts_c", data: buildEvents({ ...bodySet, duration: b.duration }, { ...headSet, duration: h.duration }) });
  return { vpk: buildVpk(files), body: b, head: h };
}
window.HS = { decodeAny, processSamples, encodeMp3, scanMp3, withInfoTag, buildVsnd, buildEvents, buildVpk, buildZip, buildPak };
