module Shop
  class Cart
    def initialize
      @items = []
    end

    def add(item)
      @items << item
      total
    end

    def total
      @items.sum
    end
  end
end
