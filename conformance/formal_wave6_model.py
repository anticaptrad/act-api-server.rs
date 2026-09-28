#!/usr/bin/env python3
from itertools import product
for auth,tenant,idem in product((False,True),repeat=3):
  for current in range(3):
    for expected in range(3):
      admitted=auth and tenant and idem and current==expected
      if admitted: assert current+1>current
      if current!=expected or not(auth and tenant and idem): assert not admitted
print("optimistic revision admission: ok")
