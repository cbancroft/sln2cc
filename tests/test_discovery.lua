-- Plugin tests stub out the CLI binary rather than requiring a real Windows toolchain,
-- so they exercise plugin logic (caching, persistence, lifecycle) only.

describe("msvclsp.discovery", function()
  it("finds the project root from configured root markers", function()
    MiniTest.skip("not yet implemented")
  end)

  it("caches discovered solutions instead of rescanning", function()
    MiniTest.skip("not yet implemented")
  end)

  it("rescan refreshes the cache", function()
    MiniTest.skip("not yet implemented")
  end)

  it("remembers the active solution per project root", function()
    MiniTest.skip("not yet implemented")
  end)
end)
