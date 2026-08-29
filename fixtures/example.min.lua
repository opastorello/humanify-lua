local function a(e,t)local n={}local r=#e local i=1 while i<=r do local j=i+t-1 if j>r then j=r end table.insert(n,e:sub(i,j)) i=i+t end return n end
